/* Contract: execve passes argv and envp correctly to new process image. */
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

int main(int argc, char *argv[]) {
    /* Child mode: verify arguments and environment */
    if (argc >= 2 && strcmp(argv[1], "--child") == 0) {
        if (argc != 3) {
            printf("CONTRACT_FAIL child_argc: argc=%d expected=3\n", argc);
            return 1;
        }
        if (strcmp(argv[2], "hello") != 0) {
            printf("CONTRACT_FAIL child_argv2: got='%s'\n", argv[2]);
            return 1;
        }
        const char *env = getenv("CONTRACT_ENV");
        if (env == NULL || strcmp(env, "world") != 0) {
            printf("CONTRACT_FAIL child_env: got='%s'\n", env ? env : "(null)");
            return 1;
        }
        printf("child_verify: ok argc=%d argv2='%s' env='%s'\n", argc, argv[2], env);
        printf("CONTRACT_PASS\n");
        return 0;
    }

    /* Parent mode: exec self with arguments */
    printf("parent: execing self\n");
    char *new_argv[] = {argv[0], "--child", "hello", NULL};
    char *new_envp[] = {"CONTRACT_ENV=world", NULL};
    execve(argv[0], new_argv, new_envp);

    /* If we get here, execve failed */
    printf("CONTRACT_FAIL execve: errno=%d\n", errno);
    return 1;
}
