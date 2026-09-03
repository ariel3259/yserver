/*
 * ipcbench — bounds the IPC term of the Phase C.0 KmsIoExecutor coordinate path.
 *
 * Models the owner/executor split: a single-threaded "core" parent that must
 * hand every KMS host call to a separate process over message-boundary-
 * preserving IPC, and wait for the typed reply before dispatching anything
 * else (spec section 6.3, COMMIT-5).
 *
 * It measures ONLY the transport cost. It does not perform an ioctl, and it
 * does not include the real input path; those need instrumented yserver on a
 * VT. What it answers is whether the transport alone can fit under the
 * section 16.3 budget, under the load condition where the cursor actually
 * moves — a busy core, not an idle box.
 *
 * Per iteration, an "input" is due at a fixed cadence:
 *   t_due      the coordinate update became available
 *   t_dispatch sendmsg() returned in the core
 *   t_reply    the typed reply was received
 * reporting due->dispatch (core queueing + wakeup), dispatch->reply (IPC RTT)
 * and due->reply (what the coordinate path actually costs before the ioctl).
 */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

/* Coordinate request: incarnation, epoch, plane, cursor generation, contract
 * proof, hazard classification, coordinates. Reply: typed result plus the
 * helper-measured ioctl duration. Sizes are the shape, not the semantics. */
#define REQ_BYTES 64
#define REP_BYTES 32

static inline uint64_t now_ns(void)
{
	struct timespec ts;
	clock_gettime(CLOCK_MONOTONIC, &ts);
	return (uint64_t)ts.tv_sec * 1000000000ull + ts.tv_nsec;
}

static void spin_ns(uint64_t ns)
{
	uint64_t end = now_ns() + ns;
	while (now_ns() < end)
		;
}

static int cmp_u64(const void *a, const void *b)
{
	uint64_t x = *(const uint64_t *)a, y = *(const uint64_t *)b;
	return (x > y) - (x < y);
}

static uint64_t pct(uint64_t *sorted, size_t n, double p)
{
	if (!n)
		return 0;
	size_t i = (size_t)(p * (double)(n - 1) + 0.5);
	return sorted[i];
}

/* Executor helper: receive one framed request, optionally burn the configured
 * ioctl cost, reply. Never batches, never reorders. */
static void helper(int fd, uint64_t ioctl_ns, int pass_fd)
{
	char rbuf[512], sbuf[REP_BYTES];
	int devnull = pass_fd ? open("/dev/null", O_RDONLY) : -1;
	union {
		struct cmsghdr align;
		char buf[CMSG_SPACE(sizeof(int))];
	} cm;

	memset(sbuf, 0, sizeof(sbuf));
	for (;;) {
		ssize_t n = recv(fd, rbuf, sizeof(rbuf), 0);
		if (n <= 0)
			break;
		if (ioctl_ns)
			spin_ns(ioctl_ns);

		struct iovec iov = { .iov_base = sbuf, .iov_len = sizeof(sbuf) };
		struct msghdr msg = { .msg_iov = &iov, .msg_iovlen = 1 };
		if (pass_fd) {
			memset(&cm, 0, sizeof(cm));
			msg.msg_control = cm.buf;
			msg.msg_controllen = sizeof(cm.buf);
			struct cmsghdr *c = CMSG_FIRSTHDR(&msg);
			c->cmsg_level = SOL_SOCKET;
			c->cmsg_type = SCM_RIGHTS;
			c->cmsg_len = CMSG_LEN(sizeof(int));
			memcpy(CMSG_DATA(c), &devnull, sizeof(int));
		}
		if (sendmsg(fd, &msg, 0) < 0)
			break;
	}
	_exit(0);
}

int main(int argc, char **argv)
{
	unsigned hz = 1000, iters = 20000, spinners = 0;
	uint64_t work_ns = 0, ioctl_ns = 0;
	int pass_fd = 0, spin_wait = 0;
	const char *label = "run";

	for (int i = 1; i < argc; i++) {
		if (!strcmp(argv[i], "--hz")) hz = atoi(argv[++i]);
		else if (!strcmp(argv[i], "--iters")) iters = atoi(argv[++i]);
		else if (!strcmp(argv[i], "--work-us")) work_ns = 1000ull * atoi(argv[++i]);
		else if (!strcmp(argv[i], "--ioctl-us")) ioctl_ns = 1000ull * atoi(argv[++i]);
		else if (!strcmp(argv[i], "--spinners")) spinners = atoi(argv[++i]);
		else if (!strcmp(argv[i], "--pass-fd")) pass_fd = 1;
		else if (!strcmp(argv[i], "--spin-wait")) spin_wait = 1;
		else if (!strcmp(argv[i], "--label")) label = argv[++i];
		else { fprintf(stderr, "unknown arg %s\n", argv[i]); return 2; }
	}

	int sv[2];
	if (socketpair(AF_UNIX, SOCK_SEQPACKET, 0, sv) < 0) {
		perror("socketpair");
		return 1;
	}

	pid_t child = fork();
	if (child == 0) {
		close(sv[0]);
		helper(sv[1], ioctl_ns, pass_fd);
	}
	close(sv[1]);

	/* Unrelated system contention, so the measurement is not of an idle box. */
	pid_t *sp = calloc(spinners ? spinners : 1, sizeof(pid_t));
	for (unsigned i = 0; i < spinners; i++) {
		pid_t p = fork();
		if (p == 0) {
			for (;;)
				;
		}
		sp[i] = p;
	}

	uint64_t *d_dispatch = malloc(iters * sizeof(uint64_t));
	uint64_t *d_rtt = malloc(iters * sizeof(uint64_t));
	uint64_t *d_total = malloc(iters * sizeof(uint64_t));
	if (!d_dispatch || !d_rtt || !d_total)
		return 1;

	char req[REQ_BYTES], rep[512];
	union {
		struct cmsghdr align;
		char buf[CMSG_SPACE(sizeof(int))];
	} cm;
	memset(req, 0, sizeof(req));

	uint64_t period = 1000000000ull / hz;
	uint64_t t0 = now_ns();
	unsigned late = 0;

	/* Warmup, excluded. */
	for (int i = 0; i < 200; i++) {
		send(sv[0], req, sizeof(req), 0);
		struct iovec iov = { .iov_base = rep, .iov_len = sizeof(rep) };
		struct msghdr m = { .msg_iov = &iov, .msg_iovlen = 1,
				    .msg_control = cm.buf, .msg_controllen = sizeof(cm.buf) };
		recvmsg(sv[0], &m, 0);
		for (struct cmsghdr *c = CMSG_FIRSTHDR(&m); c; c = CMSG_NXTHDR(&m, c))
			if (c->cmsg_type == SCM_RIGHTS) { int f; memcpy(&f, CMSG_DATA(c), sizeof(f)); close(f); }
	}

	t0 = now_ns();
	for (unsigned i = 0; i < iters; i++) {
		uint64_t due = t0 + (uint64_t)i * period;

		/* The core is busy with its own work, then waits for the input. */
		if (work_ns)
			spin_ns(work_ns);
		if (spin_wait) {
			while (now_ns() < due)
				;
		} else {
			struct timespec ts = { .tv_sec = due / 1000000000ull,
					       .tv_nsec = due % 1000000000ull };
			clock_nanosleep(CLOCK_MONOTONIC, TIMER_ABSTIME, &ts, NULL);
		}
		uint64_t t_ready = now_ns();
		if (t_ready > due + period)
			late++;

		send(sv[0], req, sizeof(req), 0);
		uint64_t t_disp = now_ns();

		struct iovec iov = { .iov_base = rep, .iov_len = sizeof(rep) };
		struct msghdr m = { .msg_iov = &iov, .msg_iovlen = 1,
				    .msg_control = cm.buf, .msg_controllen = sizeof(cm.buf) };
		if (recvmsg(sv[0], &m, 0) <= 0)
			break;
		for (struct cmsghdr *c = CMSG_FIRSTHDR(&m); c; c = CMSG_NXTHDR(&m, c))
			if (c->cmsg_type == SCM_RIGHTS) { int f; memcpy(&f, CMSG_DATA(c), sizeof(f)); close(f); }
		uint64_t t_rep = now_ns();

		d_dispatch[i] = t_disp - due;
		d_rtt[i] = t_rep - t_disp;
		d_total[i] = t_rep - due;
	}
	uint64_t elapsed = now_ns() - t0;

	close(sv[0]);
	kill(child, SIGKILL);
	waitpid(child, NULL, 0);
	for (unsigned i = 0; i < spinners; i++) {
		kill(sp[i], SIGKILL);
		waitpid(sp[i], NULL, 0);
	}

	qsort(d_dispatch, iters, sizeof(uint64_t), cmp_u64);
	qsort(d_rtt, iters, sizeof(uint64_t), cmp_u64);
	qsort(d_total, iters, sizeof(uint64_t), cmp_u64);

#define US(x) ((double)(x) / 1000.0)
	printf("%-22s n=%u hz=%u work=%luus ioctl=%luus spin=%u fd=%d wait=%s\n",
	       label, iters, hz, (unsigned long)(work_ns / 1000),
	       (unsigned long)(ioctl_ns / 1000), spinners, pass_fd,
	       spin_wait ? "spin" : "sleep");
	printf("  due->dispatch  p50=%7.1f p99=%8.1f p99.9=%8.1f max=%9.1f us\n",
	       US(pct(d_dispatch, iters, 0.50)), US(pct(d_dispatch, iters, 0.99)),
	       US(pct(d_dispatch, iters, 0.999)), US(d_dispatch[iters - 1]));
	printf("  IPC rtt        p50=%7.1f p99=%8.1f p99.9=%8.1f max=%9.1f us\n",
	       US(pct(d_rtt, iters, 0.50)), US(pct(d_rtt, iters, 0.99)),
	       US(pct(d_rtt, iters, 0.999)), US(d_rtt[iters - 1]));
	printf("  due->reply     p50=%7.1f p99=%8.1f p99.9=%8.1f max=%9.1f us\n",
	       US(pct(d_total, iters, 0.50)), US(pct(d_total, iters, 0.99)),
	       US(pct(d_total, iters, 0.999)), US(d_total[iters - 1]));
	printf("  achieved=%.1f updates/s  offered=%u  late-periods=%u\n\n",
	       (double)iters * 1e9 / (double)elapsed, hz, late);
	return 0;
}
