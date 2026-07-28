yserver(1)

# NAME

yserver - X11 server for Linux KMS/DRM

# SYNOPSIS

*yserver* [:_display_] [vt_N_] [-auth _file_] [-displayfd _fd_] [-layout _layout_] [--version]

# DESCRIPTION

*yserver* is an X11 server written from scratch in Rust. It is not an Xorg
derivative. It drives atomic KMS/DRM directly and renders with Vulkan.

There is no seat manager. The server process opens _/dev/dri/\*_ and
_/dev/input/event\*_ itself, so it needs read/write access to both, and it
requires a working Vulkan driver. How that access is obtained depends on
how the server is started:

*Launched by a display manager*
	The server runs as root and already has access. See
	_@DOCDIR@/examples/lightdm-99-yserver.conf_.

*Launched by the user from a console*
	Use *starty*(1). The invoking user needs the _video_ and _input_
	groups, or equivalent seat ACLs:

	```
	sudo usermod -aG video,input $USER
	```

	then log out and back in.

# OPTIONS

*:*_display_
	Display number to serve, e.g. *:1*. A bare number is also accepted.
	Defaults to *:7*.

*vt*_N_
	Use virtual console _N_, e.g. *vt7*.

*-auth* _file_
	Read the authority file used to validate connecting clients. This is
	the server's own copy; it is not the client-side _XAUTHORITY_.

*-displayfd* _fd_
	Write the chosen display number to _fd_ once the socket is listening,
	instead of taking the display from the command line.

*-layout* _layout_
	XKB layout name, e.g. *be*.

*--version*, *-version*
	Print the version and the commit it was built from, then exit.

## ACCEPTED FOR COMPATIBILITY, IGNORED

Display managers and *xinit*(1) pass these unconditionally. *yserver*
parses them so startup does not fail, but they have no effect:

*-seat* _seat_
	Parsed and discarded. Input is always assigned to *seat0*.

*-novtswitch*
	No-op.

*-nolisten* _protocol_, *-config* _file_, *-background* _spec_
	Parsed with their value and discarded. TCP is never listened on
	regardless of *-nolisten*.

# KEY BINDINGS

*Ctrl-Alt-Backspace*
	Terminate the server immediately and return to the console.

*Ctrl-Alt-F1* .. *Ctrl-Alt-F11*
	Switch to virtual console 1 to 11.

*Ctrl-Alt-Enter*
	Write the current scanout to a PPM file in the working directory.

*Ctrl-Alt-F12*
	Write every drawable's storage to PPM files in the working directory.

# SIGNALS

*SIGUSR1* (outgoing, to the parent process)
	Sent once when the server is ready to accept clients, but only if
	*SIGUSR1* was inherited with disposition *SIG_IGN* — the convention a
	display manager uses to request the readiness handshake. *lightdm*
	relies on this.

*SIGUSR1* (incoming)
	Release the virtual console. Part of the VT handover handshake, and
	unrelated to the readiness signal above.

*SIGUSR2* (incoming)
	Acquire the virtual console.

*SIGTERM*, *SIGINT*, *SIGHUP*
	Shut down, restoring the console.

The diagnostic dumps are *not* signal-driven; they are the key bindings
above.

# ENVIRONMENT

*RUST_LOG*
	Log filter. Defaults to *info*; *warn* is quieter, *debug* and *trace*
	are for diagnosis.

*YSERVER_DRM_DEVICE*
	Path to the DRM device to use instead of auto-selecting one, e.g.
	_/dev/dri/card1_.

*YSERVER_MODE*
	Force a mode instead of the connector's preferred mode, as
	_WIDTHxHEIGHT_ — for example *1920x1080*. A refresh-rate suffix is not
	accepted.

*YSERVER_HW_CURSOR*
	Set to *0*, *false*, *no* or *off* to disable the hardware cursor
	plane and composite the cursor instead. Enabled otherwise.

*YSERVER_ALLOW_SOFTWARE_VULKAN*
	If set to any value, permits a software Vulkan implementation such as
	lavapipe. Refused by default, being far too slow for interactive use.

Further *YSERVER_*\* variables exist for development tracing and are
deliberately not documented here; see the project documentation.

# FILES

_/tmp/.X11-unix/X<display>_
	The listening socket.

_@DOCDIR@/setup.md_
	Setup guide: dependencies, device access, display manager and console
	startup, troubleshooting.

_@DOCDIR@/examples/lightdm-99-yserver.conf_
	Example lightdm drop-in.

# SEE ALSO

*starty*(1), *Xserver*(1), *xauth*(1), *lightdm*(1)

# BUGS

Report at https://github.com/joske/yserver/issues
