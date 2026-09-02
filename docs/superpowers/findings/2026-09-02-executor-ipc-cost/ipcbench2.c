/*
 * ipcbench2 — measures the quantity the C.0 design actually constrains:
 * how long the owner cannot dispatch ANY host call because a coordinate
 * reservation is live, with primary traffic competing for the same channel.
 *
 * Spec section 7.1: "While the coordinate host call itself is unresolved, the
 * owner dispatches no new KMS host call." CoordinateSubmitting is a per-plane
 * reservation held from before dispatch until the typed return or the reap.
 *
 * Two request classes share one single-threaded core and one executor:
 *   coordinate  --coord-hz  (1000, input-driven), no out-fence in the reply
 *   primary     --prim-hz   (refresh-driven), reply carries an out-fence fd
 *
 * Reported per class: due->dispatch (how long the request waited for the
 * channel, which is the induced block), IPC rtt, due->reply. Plus channel
 * occupancy and how many primaries were delayed behind a live coordinate
 * reservation.
 */
#define _GNU_SOURCE
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

#define COORD 0
#define PRIM  1

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

static uint64_t pct(uint64_t *s, size_t n, double p)
{
	return n ? s[(size_t)(p * (double)(n - 1) + 0.5)] : 0;
}

struct req {
	uint32_t kind;
	char pad[60];
};

static void helper(int fd, uint64_t coord_ns, uint64_t prim_ns)
{
	char rbuf[512], sbuf[32];
	int devnull = open("/dev/null", O_RDONLY);
	union { struct cmsghdr a; char buf[CMSG_SPACE(sizeof(int))]; } cm;

	memset(sbuf, 0, sizeof(sbuf));
	for (;;) {
		ssize_t n = recv(fd, rbuf, sizeof(rbuf), 0);
		if (n <= 0)
			break;
		uint32_t kind = ((struct req *)rbuf)->kind;
		uint64_t cost = kind == COORD ? coord_ns : prim_ns;
		if (cost)
			spin_ns(cost);

		struct iovec iov = { .iov_base = sbuf, .iov_len = sizeof(sbuf) };
		struct msghdr msg = { .msg_iov = &iov, .msg_iovlen = 1 };
		if (kind == PRIM) { /* atomic success returns every out-fence by fd */
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

struct stats {
	uint64_t *disp, *rtt, *total;
	size_t n, cap;
};

static void add(struct stats *s, uint64_t d, uint64_t r, uint64_t t)
{
	if (s->n >= s->cap)
		return;
	s->disp[s->n] = d;
	s->rtt[s->n] = r;
	s->total[s->n] = t;
	s->n++;
}

static void alloc_stats(struct stats *s, size_t cap)
{
	s->cap = cap;
	s->n = 0;
	s->disp = malloc(cap * sizeof(uint64_t));
	s->rtt = malloc(cap * sizeof(uint64_t));
	s->total = malloc(cap * sizeof(uint64_t));
}

#define US(x) ((double)(x) / 1000.0)
static void report(const char *name, struct stats *s)
{
	if (!s->n) { printf("  %-11s (no samples)\n", name); return; }
	qsort(s->disp, s->n, sizeof(uint64_t), cmp_u64);
	qsort(s->rtt, s->n, sizeof(uint64_t), cmp_u64);
	qsort(s->total, s->n, sizeof(uint64_t), cmp_u64);
	printf("  %-11s n=%zu\n", name, s->n);
	printf("    due->dispatch p50=%7.1f p99=%8.1f p99.9=%8.1f max=%9.1f us\n",
	       US(pct(s->disp, s->n, .5)), US(pct(s->disp, s->n, .99)),
	       US(pct(s->disp, s->n, .999)), US(s->disp[s->n - 1]));
	printf("    IPC rtt       p50=%7.1f p99=%8.1f p99.9=%8.1f max=%9.1f us\n",
	       US(pct(s->rtt, s->n, .5)), US(pct(s->rtt, s->n, .99)),
	       US(pct(s->rtt, s->n, .999)), US(s->rtt[s->n - 1]));
	printf("    due->reply    p50=%7.1f p99=%8.1f p99.9=%8.1f max=%9.1f us\n",
	       US(pct(s->total, s->n, .5)), US(pct(s->total, s->n, .99)),
	       US(pct(s->total, s->n, .999)), US(s->total[s->n - 1]));
}

int main(int argc, char **argv)
{
	unsigned coord_hz = 1000, prim_hz = 60, spinners = 0, secs = 20;
	uint64_t coord_ns = 0, prim_ns = 0;
	const char *label = "run";

	for (int i = 1; i < argc; i++) {
		if (!strcmp(argv[i], "--coord-hz")) coord_hz = atoi(argv[++i]);
		else if (!strcmp(argv[i], "--prim-hz")) prim_hz = atoi(argv[++i]);
		else if (!strcmp(argv[i], "--coord-ioctl-us")) coord_ns = 1000ull * atoi(argv[++i]);
		else if (!strcmp(argv[i], "--prim-ioctl-us")) prim_ns = 1000ull * atoi(argv[++i]);
		else if (!strcmp(argv[i], "--spinners")) spinners = atoi(argv[++i]);
		else if (!strcmp(argv[i], "--secs")) secs = atoi(argv[++i]);
		else if (!strcmp(argv[i], "--label")) label = argv[++i];
		else { fprintf(stderr, "unknown arg %s\n", argv[i]); return 2; }
	}

	int sv[2];
	if (socketpair(AF_UNIX, SOCK_SEQPACKET, 0, sv) < 0) return 1;
	pid_t child = fork();
	if (child == 0) { close(sv[0]); helper(sv[1], coord_ns, prim_ns); }
	close(sv[1]);

	pid_t *sp = calloc(spinners ? spinners : 1, sizeof(pid_t));
	for (unsigned i = 0; i < spinners; i++) {
		pid_t p = fork();
		if (p == 0) { for (;;) ; }
		sp[i] = p;
	}

	struct stats sc, sp2;
	alloc_stats(&sc, (size_t)coord_hz * secs + 1024);
	alloc_stats(&sp2, (size_t)prim_hz * secs + 1024);

	uint64_t cper = 1000000000ull / coord_hz;
	uint64_t pper = 1000000000ull / prim_hz;
	struct req req;
	char rep[512];
	union { struct cmsghdr a; char buf[CMSG_SPACE(sizeof(int))]; } cm;
	memset(&req, 0, sizeof(req));

	/* warmup */
	for (int i = 0; i < 200; i++) {
		req.kind = COORD;
		send(sv[0], &req, sizeof(req), 0);
		recv(sv[0], rep, sizeof(rep), 0);
	}

	uint64_t t0 = now_ns(), deadline = t0 + (uint64_t)secs * 1000000000ull;
	uint64_t cdue = t0, pdue = t0, busy = 0;
	unsigned prim_behind_coord = 0;
	uint64_t coord_reservation_live_until = 0;

	while (now_ns() < deadline) {
		int kind;
		uint64_t due;
		if (cdue <= pdue) { kind = COORD; due = cdue; }
		else { kind = PRIM; due = pdue; }

		uint64_t t = now_ns();
		if (t < due) {
			struct timespec ts = { .tv_sec = due / 1000000000ull,
					       .tv_nsec = due % 1000000000ull };
			clock_nanosleep(CLOCK_MONOTONIC, TIMER_ABSTIME, &ts, NULL);
		}
		/* A primary whose deadline fell inside a live coordinate
		 * reservation could not have been dispatched then. */
		if (kind == PRIM && due < coord_reservation_live_until)
			prim_behind_coord++;

		req.kind = kind;
		send(sv[0], &req, sizeof(req), 0);
		uint64_t t_disp = now_ns();

		struct iovec iov = { .iov_base = rep, .iov_len = sizeof(rep) };
		struct msghdr m = { .msg_iov = &iov, .msg_iovlen = 1,
				    .msg_control = cm.buf, .msg_controllen = sizeof(cm.buf) };
		if (recvmsg(sv[0], &m, 0) <= 0) break;
		for (struct cmsghdr *c = CMSG_FIRSTHDR(&m); c; c = CMSG_NXTHDR(&m, c))
			if (c->cmsg_type == SCM_RIGHTS) { int f; memcpy(&f, CMSG_DATA(c), sizeof(f)); close(f); }
		uint64_t t_rep = now_ns();

		busy += t_rep - t_disp;
		if (kind == COORD) {
			coord_reservation_live_until = t_rep;
			add(&sc, t_disp - due, t_rep - t_disp, t_rep - due);
			cdue += cper;
		} else {
			add(&sp2, t_disp - due, t_rep - t_disp, t_rep - due);
			pdue += pper;
		}
	}
	uint64_t elapsed = now_ns() - t0;

	close(sv[0]);
	kill(child, SIGKILL); waitpid(child, NULL, 0);
	for (unsigned i = 0; i < spinners; i++) { kill(sp[i], SIGKILL); waitpid(sp[i], NULL, 0); }

	printf("%-20s coord=%uHz prim=%uHz coord_ioctl=%luus prim_ioctl=%luus spin=%u secs=%u\n",
	       label, coord_hz, prim_hz, (unsigned long)(coord_ns / 1000),
	       (unsigned long)(prim_ns / 1000), spinners, secs);
	report("coordinate", &sc);
	report("primary", &sp2);
	printf("    channel occupancy=%.3f%%  primaries due inside a live coord reservation=%u/%zu\n\n",
	       100.0 * (double)busy / (double)elapsed, prim_behind_coord, sp2.n);
	return 0;
}
