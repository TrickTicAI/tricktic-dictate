// Test-only dyld interposer. The controller supplies an exact disposable path.
// Never load this into Writer or a system process.
#include <fcntl.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

static int fixture_rename(const char *old_path, const char *new_path) {
    int result = rename(old_path, new_path);
    const char *target = getenv("WRITER_INSTALL_STOP_PATH");
    const char *receipt = getenv("WRITER_INSTALL_STOP_RECEIPT");
    const char *root = getenv("WRITER_INSTALL_FIXTURE_ROOT");
    if (result == 0 && target && receipt && root && strcmp(old_path, target) == 0) {
        // Refuse to signal outside this controller's private temporary tree.
        size_t n = strlen(root);
        if (strncmp(new_path, root, n) || new_path[n] != '/') _exit(91);
        int fd = open(receipt, O_WRONLY | O_CREAT | O_EXCL, 0600);
        if (fd < 0) _exit(92);
        size_t length = strlen(new_path);
        if (write(fd, new_path, length) != (ssize_t)length || fsync(fd) != 0) _exit(93);
        close(fd);
        raise(SIGSTOP);
        _exit(94); // Controller must kill; never continue after the stop point.
    }
    return result;
}
__attribute__((used)) static struct { const void *replacement; const void *original; }
fixture_interpose __attribute__((section("__DATA,__interpose"))) = {
    (const void *)fixture_rename, (const void *)rename
};
