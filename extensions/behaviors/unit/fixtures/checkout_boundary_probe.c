/* Issue 26 capability experiment, not a production checkout guard.
 * Restrict filesystem mutations with Landlock ABI 3; leave reads, execution,
 * IPC, and networking unrestricted to isolate the checkout-identity problem.
 * Build and drive only through checkout_boundary_probe_test.ts.
 */
#define _GNU_SOURCE
#include <fcntl.h>
#include <linux/landlock.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/prctl.h>
#include <sys/syscall.h>
#include <unistd.h>

static int fail(const char *stage) {
    perror(stage);
    return 125;
}

static int allow(int ruleset, const char *path, unsigned long long rights) {
    int directory = open(path, O_PATH | O_CLOEXEC);
    if (directory < 0) return fail(path);
    struct landlock_path_beneath_attr rule = {
        .allowed_access = rights,
        .parent_fd = directory,
    };
    int result = syscall(SYS_landlock_add_rule, ruleset,
                         LANDLOCK_RULE_PATH_BENEATH, &rule, 0);
    close(directory);
    return result < 0 ? fail("landlock_add_rule") : 0;
}

int main(int argc, char **argv) {
    int abi = syscall(SYS_landlock_create_ruleset, NULL, 0,
                      LANDLOCK_CREATE_RULESET_VERSION);
    if (abi < 0) return fail("Landlock unavailable");
    if (argc == 2 && strcmp(argv[1], "--abi") == 0) {
        printf("%d\n", abi);
        return 0;
    }
    if (abi < 3) {
        fputs("Landlock ABI 3 required for truncation restrictions\n", stderr);
        return 125;
    }
    if (argc < 5) {
        fputs("usage: probe ROOT SCRATCH EXTRA_WRITABLE_OR_DASH COMMAND [ARGS...]\n",
              stderr);
        return 125;
    }
    if (chdir(argv[1]) < 0) return fail("assigned cwd unavailable");
    if (setenv("TMPDIR", argv[2], 1) < 0) return fail("private scratch environment");
    const unsigned long long mutations =
        LANDLOCK_ACCESS_FS_WRITE_FILE | LANDLOCK_ACCESS_FS_REMOVE_DIR |
        LANDLOCK_ACCESS_FS_REMOVE_FILE | LANDLOCK_ACCESS_FS_MAKE_CHAR |
        LANDLOCK_ACCESS_FS_MAKE_DIR | LANDLOCK_ACCESS_FS_MAKE_REG |
        LANDLOCK_ACCESS_FS_MAKE_SOCK | LANDLOCK_ACCESS_FS_MAKE_FIFO |
        LANDLOCK_ACCESS_FS_MAKE_BLOCK | LANDLOCK_ACCESS_FS_MAKE_SYM |
        LANDLOCK_ACCESS_FS_REFER | LANDLOCK_ACCESS_FS_TRUNCATE;
    struct landlock_ruleset_attr attributes = {.handled_access_fs = mutations};
    int ruleset = syscall(SYS_landlock_create_ruleset, &attributes,
                          sizeof(attributes), 0);
    if (ruleset < 0) return fail("landlock_create_ruleset");
    if (allow(ruleset, argv[1], mutations) ||
        allow(ruleset, argv[2], mutations) ||
        allow(ruleset, "/dev/null",
              LANDLOCK_ACCESS_FS_WRITE_FILE | LANDLOCK_ACCESS_FS_TRUNCATE) ||
        (strcmp(argv[3], "-") != 0 && allow(ruleset, argv[3], mutations))) {
        close(ruleset);
        return 125;
    }
    if (prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) < 0)
        return fail("PR_SET_NO_NEW_PRIVS");
    if (syscall(SYS_landlock_restrict_self, ruleset, 0) < 0)
        return fail("landlock_restrict_self");
    close(ruleset);
    execvp(argv[4], &argv[4]);
    return fail("execvp");
}
