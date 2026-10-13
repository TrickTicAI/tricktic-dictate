// Private process-group supervisor. kqueue observes the app's death even after
// SIGKILL; no persisted PID can accidentally target an unrelated process later.
#include <sys/event.h>
#include <sys/wait.h>
#include <sys/resource.h>
#include <sys/stat.h>
#include <spawn.h>
#include <signal.h>
#include <unistd.h>
#include <stdlib.h>
#include <errno.h>
extern char **environ;
static void stop(int signal_number) {
    (void)signal_number;
    killpg(getpgrp(), SIGKILL);
    _exit(125);
}
int main(int argc, char **argv) {
    if (argc < 2) return 64;
    // Also apply to direct supervisor launches. Do not allow ordinary core files
    // containing audio/prompts; OS crash reporting is a separate platform policy.
    struct rlimit core = {0, 0};
    if (setrlimit(RLIMIT_CORE, &core) != 0) return 125;
    umask(0077);
    pid_t parent = getppid();
    if (parent <= 1 || setpgid(0, 0) != 0) return 125;
    signal(SIGTERM, stop);
    signal(SIGINT, stop);
    signal(SIGHUP, stop);
    int queue = kqueue();
    if (queue < 0) return 125;
    struct kevent event;
    EV_SET(&event, parent, EVFILT_PROC, EV_ADD | EV_ONESHOT, NOTE_EXIT, 0, NULL);
    if (kevent(queue, &event, 1, NULL, 0, NULL) < 0 || getppid() != parent) return 125;
    pid_t child;
    if (posix_spawn(&child, argv[1], NULL, NULL, argv + 1, environ) != 0) return 126;
    // Poll waitpid too: a child can exit before its NOTE_EXIT registration.
    for (;;) {
        int status;
        pid_t result = waitpid(child, &status, WNOHANG);
        if (result == child) {
            // Reap any surviving descendants before publishing worker exit.
            int code = WIFEXITED(status) ? WEXITSTATUS(status) : 128 + WTERMSIG(status);
            // Ignore TERM in the supervisor while terminating its private group.
            signal(SIGTERM, SIG_IGN);
            killpg(getpgrp(), SIGTERM);
            _exit(code);
        }
        if (result < 0 && errno != EINTR) stop(0);
        struct timespec timeout = {0, 100000000};
        int count = kevent(queue, NULL, 0, &event, 1, &timeout);
        if (count > 0 || (count < 0 && errno != EINTR)) stop(0);
    }
}
