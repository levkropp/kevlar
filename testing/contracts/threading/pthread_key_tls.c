/* Contract: pthread_key TLS is per-thread; no cross-thread contamination. */
#include <errno.h>
#include <pthread.h>
#include <stdio.h>

static pthread_key_t key;
static volatile int thread_saw = 0;

static void *thread_fn(void *arg) {
    (void)arg;
    pthread_setspecific(key, (void *)99);
    long val = (long)pthread_getspecific(key);
    thread_saw = (int)val;
    return NULL;
}

int main(void) {
    if (pthread_key_create(&key, NULL) != 0) {
        printf("CONTRACT_FAIL key_create: errno=%d\n", errno);
        return 1;
    }

    /* Main thread sets key = 42 */
    pthread_setspecific(key, (void *)42);
    printf("main_set: ok val=42\n");

    /* Spawn thread that sets key = 99 */
    pthread_t tid;
    if (pthread_create(&tid, NULL, thread_fn, NULL) != 0) {
        printf("CONTRACT_FAIL pthread_create: errno=%d\n", errno);
        return 1;
    }
    pthread_join(tid, NULL);

    /* Thread should have seen 99 */
    if (thread_saw != 99) {
        printf("CONTRACT_FAIL thread_val: got=%d expected=99\n", thread_saw);
        return 1;
    }
    printf("thread_val: ok got=99\n");

    /* Main thread should still see 42 */
    long main_val = (long)pthread_getspecific(key);
    if (main_val != 42) {
        printf("CONTRACT_FAIL main_val: got=%ld expected=42\n", main_val);
        return 1;
    }
    printf("main_val: ok got=42\n");

    pthread_key_delete(key);
    printf("CONTRACT_PASS\n");
    return 0;
}
