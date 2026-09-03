//! Framed transport with out-fence fd passing for process-isolated executor host calls.

use std::{
    io,
    mem::MaybeUninit,
    os::{
        fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd},
        unix::net::UnixStream,
    },
};

use super::protocol::{HostCallReply, decode_reply};

#[allow(dead_code)] // Will be consumed in Task 9, 10
pub(crate) const MAX_OUT_FENCES: usize = 16;

#[allow(dead_code)] // Will be consumed in Task 9, 10
pub(crate) const REPLY_FRAME_LEN: usize = super::protocol::REPLY_FRAME_LEN;

#[allow(dead_code)] // Will be consumed in Task 9, 10
#[derive(Debug)]
pub(crate) struct ReceivedFrame {
    pub(crate) len: usize,
    pub(crate) fds: Vec<OwnedFd>,
}

#[repr(C)]
union CmsgStorage {
    buf: [u8; 256],
    _align: libc::cmsghdr,
}

/// Create a connected pair of UNIX domain sockets using `SOCK_SEQPACKET`.
///
/// `SOCK_SEQPACKET` preserves message boundaries while providing reliable,
/// sequenced datagram delivery and descriptor passing.
#[allow(dead_code)] // Will be consumed in Task 9
pub(crate) fn seqpacket_pair() -> io::Result<(UnixStream, UnixStream)> {
    let mut fds = [0 as libc::c_int; 2];
    // SAFETY: socketpair writes exactly two descriptors into `fds`.
    let rc = unsafe {
        libc::socketpair(
            libc::AF_UNIX,
            libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC,
            0,
            fds.as_mut_ptr(),
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: both descriptors are freshly created and owned here.
    unsafe {
        Ok((
            UnixStream::from_raw_fd(fds[0]),
            UnixStream::from_raw_fd(fds[1]),
        ))
    }
}

/// Send a frame without file descriptors.
#[allow(dead_code)] // Will be consumed in Task 9
pub(crate) fn send_frame(socket: &UnixStream, frame: &[u8]) -> io::Result<()> {
    send_reply_with_fences(socket, frame, &[])
}

/// Send a frame along with borrowed out-fence file descriptors via `SCM_RIGHTS`.
#[allow(dead_code)] // Will be consumed in Task 10
pub(crate) fn send_reply_with_fences(
    socket: &UnixStream,
    frame: &[u8],
    fences: &[BorrowedFd<'_>],
) -> io::Result<()> {
    if fences.len() > MAX_OUT_FENCES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "fence count exceeds MAX_OUT_FENCES",
        ));
    }

    let mut iov = libc::iovec {
        iov_base: frame.as_ptr() as *mut _,
        iov_len: frame.len() as _,
    };

    let mut msg: libc::msghdr = unsafe { MaybeUninit::zeroed().assume_init() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1 as _;

    let mut cmsg_storage = CmsgStorage { buf: [0u8; 256] };

    if !fences.is_empty() {
        let fds_bytes = fences.len() * std::mem::size_of::<libc::c_int>();
        let cmsg_len = unsafe { libc::CMSG_LEN(fds_bytes as libc::c_uint) } as usize;
        let cmsg_space = unsafe { libc::CMSG_SPACE(fds_bytes as libc::c_uint) } as usize;
        debug_assert!(cmsg_space <= std::mem::size_of::<CmsgStorage>());

        msg.msg_control = unsafe { cmsg_storage.buf.as_mut_ptr().cast() };
        msg.msg_controllen = cmsg_space as _;

        let cmsg = unsafe { libc::CMSG_FIRSTHDR(&msg) };
        if cmsg.is_null() {
            return Err(io::Error::other("CMSG_FIRSTHDR returned null pointer"));
        }

        unsafe {
            (*cmsg).cmsg_level = libc::SOL_SOCKET;
            (*cmsg).cmsg_type = libc::SCM_RIGHTS;
            (*cmsg).cmsg_len = cmsg_len as _;

            let data = libc::CMSG_DATA(cmsg);
            for (i, fence) in fences.iter().enumerate() {
                let raw_fd = fence.as_raw_fd();
                std::ptr::copy_nonoverlapping(
                    (&raw_fd as *const libc::c_int).cast::<u8>(),
                    data.add(i * std::mem::size_of::<libc::c_int>()),
                    std::mem::size_of::<libc::c_int>(),
                );
            }
        }
    }

    loop {
        let rc = unsafe { libc::sendmsg(socket.as_raw_fd(), &msg, libc::MSG_NOSIGNAL) };
        if rc < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(err);
        }
        if (rc as usize) < frame.len() {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "failed to send complete frame",
            ));
        }
        return Ok(());
    }
}

/// Receive a frame from `socket`, adopting any received `SCM_RIGHTS` descriptors into `OwnedFd`.
#[allow(dead_code)] // Will be consumed in Task 9, 10
pub(crate) fn recv_frame(socket: &UnixStream, frame: &mut [u8]) -> io::Result<ReceivedFrame> {
    let mut iov = libc::iovec {
        iov_base: frame.as_mut_ptr().cast(),
        iov_len: frame.len() as _,
    };

    let mut cmsg_storage = CmsgStorage { buf: [0u8; 256] };
    let cmsg_space = unsafe {
        libc::CMSG_SPACE((MAX_OUT_FENCES * std::mem::size_of::<libc::c_int>()) as libc::c_uint)
    } as usize;
    debug_assert!(cmsg_space <= std::mem::size_of::<CmsgStorage>());

    let mut msg: libc::msghdr = unsafe { MaybeUninit::zeroed().assume_init() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1 as _;
    msg.msg_control = unsafe { cmsg_storage.buf.as_mut_ptr().cast() };
    msg.msg_controllen = cmsg_space as _;

    let n = loop {
        let rc = unsafe { libc::recvmsg(socket.as_raw_fd(), &mut msg, libc::MSG_CMSG_CLOEXEC) };
        if rc < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(err);
        }
        break rc;
    };

    if n == 0 {
        return Ok(ReceivedFrame {
            len: 0,
            fds: Vec::new(),
        });
    }

    let mut fds = Vec::new();
    let mut cmsg = unsafe { libc::CMSG_FIRSTHDR(&msg) };
    while !cmsg.is_null() {
        let header = unsafe { &*cmsg };
        if header.cmsg_level == libc::SOL_SOCKET && header.cmsg_type == libc::SCM_RIGHTS {
            let data = unsafe { libc::CMSG_DATA(cmsg) };
            let header_len = unsafe { libc::CMSG_LEN(0) } as usize;
            let payload_len = (header.cmsg_len as usize).saturating_sub(header_len);
            let count = payload_len / std::mem::size_of::<libc::c_int>();
            for i in 0..count {
                let mut raw_fd: libc::c_int = -1;
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        data.add(i * std::mem::size_of::<libc::c_int>()),
                        (&mut raw_fd as *mut libc::c_int).cast(),
                        std::mem::size_of::<libc::c_int>(),
                    );
                    fds.push(OwnedFd::from_raw_fd(raw_fd));
                }
            }
        }
        cmsg = unsafe { libc::CMSG_NXTHDR(&msg, cmsg) };
    }

    if (msg.msg_flags & libc::MSG_CTRUNC) != 0 {
        drop(fds);
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "control message truncated (fds dropped)",
        ));
    }

    if (msg.msg_flags & libc::MSG_TRUNC) != 0 {
        drop(fds);
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame payload truncated",
        ));
    }

    Ok(ReceivedFrame {
        len: n as usize,
        fds,
    })
}

/// Decodes `frame` as a reply and verifies that the number of attached `fds` matches
/// the declared `out_fence_count`.
///
/// Returns an error if decoding fails, or if fence count does not match the reply.
/// In case of error, `fds` is consumed and dropped, closing all descriptors.
#[allow(dead_code)] // Will be consumed in Task 9
pub(crate) fn adopt_reply(
    frame: &[u8],
    fds: Vec<OwnedFd>,
) -> io::Result<(HostCallReply, Vec<OwnedFd>)> {
    let reply = decode_reply(frame).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("malformed reply frame: {err:?}"),
        )
    })?;

    match reply {
        HostCallReply::Accepted {
            out_fence_count, ..
        } => {
            if fds.len() != out_fence_count as usize {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "out_fence_count mismatch: declared {}, received {}",
                        out_fence_count,
                        fds.len()
                    ),
                ));
            }
        }
        HostCallReply::Rejected { .. } | HostCallReply::ClockProbe { .. } => {
            if !fds.is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "unexpected fds attached to non-Accepted reply: received {}",
                        fds.len()
                    ),
                ));
            }
        }
    }

    Ok((reply, fds))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::AsFd;

    fn reply_frame_declaring(out_fence_count: u8) -> [u8; super::REPLY_FRAME_LEN] {
        let reply = crate::kms::executor::protocol::HostCallReply::Accepted {
            seq: crate::kms::executor::protocol::RequestSeq::for_tests(1),
            helper_duration_ns: 100,
            out_fence_count,
        };
        crate::kms::executor::protocol::encode_reply(&reply)
    }

    #[test]
    fn a_frame_round_trips_with_no_fds() {
        let (a, b) = seqpacket_pair().expect("pair");
        send_frame(&a, &[1, 2, 3, 4]).expect("send");
        let mut buf = [0u8; 4];
        let received = recv_frame(&b, &mut buf).expect("recv");
        assert_eq!(received.len, 4);
        assert_eq!(buf, [1, 2, 3, 4]);
        assert!(received.fds.is_empty());
    }

    #[test]
    fn two_out_fences_arrive_with_their_reply() {
        let (a, b) = seqpacket_pair().expect("pair");
        let first = std::fs::File::open("/dev/null").expect("open");
        let second = std::fs::File::open("/dev/null").expect("open");
        send_reply_with_fences(&a, &[9; 8], &[first.as_fd(), second.as_fd()]).expect("send");
        let mut buf = [0u8; 8];
        let received = recv_frame(&b, &mut buf).expect("recv");
        assert_eq!(received.fds.len(), 2, "both out-fences must arrive");
    }

    #[test]
    fn a_reply_declaring_more_fences_than_it_carries_is_rejected() {
        // No MSG_CTRUNC is set in this case: the control message is intact and
        // simply carries fewer descriptors than the payload declares. Without
        // an explicit equality check this becomes a missing completion later,
        // which is the symptom hardest to trace back to its cause.
        let (a, b) = seqpacket_pair().expect("pair");
        let only = std::fs::File::open("/dev/null").expect("open");
        send_reply_with_fences(&a, &reply_frame_declaring(2), &[only.as_fd()]).expect("send");
        let mut buf = [0u8; super::REPLY_FRAME_LEN];
        let received = recv_frame(&b, &mut buf).expect("recv");
        assert!(
            super::adopt_reply(&buf, received.fds).is_err(),
            "declared out_fence_count must equal the fds received"
        );
    }

    #[test]
    fn a_message_boundary_is_preserved_between_two_sends() {
        let (a, b) = seqpacket_pair().expect("pair");
        send_frame(&a, &[1; 8]).expect("send");
        send_frame(&a, &[2; 8]).expect("send");
        let mut buf = [0u8; 8];
        recv_frame(&b, &mut buf).expect("recv");
        assert_eq!(buf, [1; 8], "the first message must not absorb the second");
        recv_frame(&b, &mut buf).expect("recv");
        assert_eq!(buf, [2; 8]);
    }

    #[test]
    fn a_closed_peer_reports_eof_rather_than_an_empty_success() {
        let (a, b) = seqpacket_pair().expect("pair");
        drop(a);
        let mut buf = [0u8; 8];
        let result = recv_frame(&b, &mut buf);
        assert!(result.is_err() || result.expect("ok").len == 0);
    }

    #[test]
    fn a_reply_declaring_fewer_fences_than_it_carries_is_rejected() {
        let (a, b) = seqpacket_pair().expect("pair");
        let f1 = std::fs::File::open("/dev/null").expect("open");
        let f2 = std::fs::File::open("/dev/null").expect("open");
        send_reply_with_fences(&a, &reply_frame_declaring(1), &[f1.as_fd(), f2.as_fd()])
            .expect("send");
        let mut buf = [0u8; super::REPLY_FRAME_LEN];
        let received = recv_frame(&b, &mut buf).expect("recv");
        assert!(
            super::adopt_reply(&buf, received.fds).is_err(),
            "declared out_fence_count fewer than carried must fail"
        );
    }

    #[test]
    fn a_rejected_reply_carrying_fences_is_rejected() {
        let reply = crate::kms::executor::protocol::HostCallReply::Rejected {
            seq: crate::kms::executor::protocol::RequestSeq::for_tests(1),
            errno: libc::EBUSY,
            helper_duration_ns: 50,
        };
        let frame = crate::kms::executor::protocol::encode_reply(&reply);
        let (a, b) = seqpacket_pair().expect("pair");
        let f = std::fs::File::open("/dev/null").expect("open");
        send_reply_with_fences(&a, &frame, &[f.as_fd()]).expect("send");
        let mut buf = [0u8; super::REPLY_FRAME_LEN];
        let received = recv_frame(&b, &mut buf).expect("recv");
        assert!(
            super::adopt_reply(&buf, received.fds).is_err(),
            "rejected reply cannot carry fds"
        );
    }

    #[test]
    fn a_clock_probe_reply_carrying_fences_is_rejected() {
        let reply = crate::kms::executor::protocol::HostCallReply::ClockProbe {
            seq: crate::kms::executor::protocol::RequestSeq::for_tests(1),
            sequence: 12345,
            helper_duration_ns: 50,
        };
        let frame = crate::kms::executor::protocol::encode_reply(&reply);
        let (a, b) = seqpacket_pair().expect("pair");
        let f = std::fs::File::open("/dev/null").expect("open");
        send_reply_with_fences(&a, &frame, &[f.as_fd()]).expect("send");
        let mut buf = [0u8; super::REPLY_FRAME_LEN];
        let received = recv_frame(&b, &mut buf).expect("recv");
        assert!(
            super::adopt_reply(&buf, received.fds).is_err(),
            "clock probe reply cannot carry fds"
        );
    }

    #[test]
    fn adopt_reply_with_matching_fences_succeeds() {
        let (a, b) = seqpacket_pair().expect("pair");
        let f1 = std::fs::File::open("/dev/null").expect("open");
        let f2 = std::fs::File::open("/dev/null").expect("open");
        send_reply_with_fences(&a, &reply_frame_declaring(2), &[f1.as_fd(), f2.as_fd()])
            .expect("send");
        let mut buf = [0u8; super::REPLY_FRAME_LEN];
        let received = recv_frame(&b, &mut buf).expect("recv");
        let (reply, fds) = super::adopt_reply(&buf, received.fds).expect("adopt");
        assert_eq!(fds.len(), 2);
        match reply {
            crate::kms::executor::protocol::HostCallReply::Accepted {
                out_fence_count, ..
            } => assert_eq!(out_fence_count, 2),
            other => panic!("expected Accepted, got {other:?}"),
        }
    }

    #[test]
    fn sending_more_than_max_out_fences_is_rejected() {
        let (a, _b) = seqpacket_pair().expect("pair");
        let files: Vec<_> = (0..MAX_OUT_FENCES + 1)
            .map(|_| std::fs::File::open("/dev/null").expect("open"))
            .collect();
        let borrowed: Vec<_> = files.iter().map(|f| f.as_fd()).collect();
        let err = send_reply_with_fences(&a, &[1, 2, 3, 4], &borrowed);
        assert!(err.is_err());
        assert_eq!(err.unwrap_err().kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn truncated_control_message_returns_invalid_data_error() {
        let (a, b) = seqpacket_pair().expect("pair");
        let files: Vec<_> = (0..MAX_OUT_FENCES + 1)
            .map(|_| std::fs::File::open("/dev/null").expect("open"))
            .collect();
        let raw_fds: Vec<libc::c_int> = files.iter().map(|f| f.as_raw_fd()).collect();

        // Send raw SCM_RIGHTS with MAX_OUT_FENCES + 1 fds directly using sendmsg
        let data = [42u8; 4];
        let mut iov = libc::iovec {
            iov_base: data.as_ptr() as *mut _,
            iov_len: data.len() as _,
        };

        #[repr(C)]
        union LargeCmsg {
            buf: [u8; 512],
            _align: libc::cmsghdr,
        }
        let mut cmsg_storage = LargeCmsg { buf: [0u8; 512] };
        let fds_bytes = raw_fds.len() * std::mem::size_of::<libc::c_int>();
        let cmsg_len = unsafe { libc::CMSG_LEN(fds_bytes as libc::c_uint) } as usize;
        let cmsg_space = unsafe { libc::CMSG_SPACE(fds_bytes as libc::c_uint) } as usize;

        let mut msg: libc::msghdr = unsafe { MaybeUninit::zeroed().assume_init() };
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1 as _;
        msg.msg_control = unsafe { cmsg_storage.buf.as_mut_ptr().cast() };
        msg.msg_controllen = cmsg_space as _;

        let cmsg = unsafe { libc::CMSG_FIRSTHDR(&msg) };
        assert!(!cmsg.is_null());
        unsafe {
            (*cmsg).cmsg_level = libc::SOL_SOCKET;
            (*cmsg).cmsg_type = libc::SCM_RIGHTS;
            (*cmsg).cmsg_len = cmsg_len as _;
            let cmsg_data = libc::CMSG_DATA(cmsg);
            std::ptr::copy_nonoverlapping(raw_fds.as_ptr().cast::<u8>(), cmsg_data, fds_bytes);
            let rc = libc::sendmsg(a.as_raw_fd(), &msg, 0);
            assert!(rc > 0);
        }

        // recv_frame only sizes control buffer for MAX_OUT_FENCES, so the kernel
        // must set MSG_CTRUNC and recv_frame must return InvalidData.
        let mut buf = [0u8; 4];
        let err = recv_frame(&b, &mut buf).expect_err("must fail due to MSG_CTRUNC");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("control message truncated"));
    }
}
