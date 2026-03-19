/* Contract: pthread_mutex protects shared counter across 4 threads. */
#include <errno.h>
#include <pthread.h>
#include <stdio.h>

#define NUM_THREADS  4
#define INCREMENTS   1000

static pthread_mutex_t mtx = PTHREAD_MUTEX_INITIALIZER;
static int counter = 0;

static void *thread_fn(void *arg) {
    (void)arg;
    for (int i = 0; i < INCREMENTS; i++) {
        pthread_mutex_lock(&mtx);
        counter++;
        pthread_mutex_unlock(&mtx);
    }
    return NULL;
}

int main(void) {
    pthread_t threads[NUM_THREADS];

    for (int i = 0; i < NUM_THREADS; i++) {
        if (pthread_create(&threads[i], NULL, thread_fn, NULL) != 0) {
            printf("CONTRACT_FAIL pthread_create: i=%d errno=%d\n", i, errno);
            return 1;
        }
    }

    for (int i = 0; i < NUM_THREADS; i++) {
        pthread_join(threads[i], NULL);
    }

    int expected = NUM_THREADS * INCREMENTS;
    if (counter != expected) {
        printf("CONTRACT_FAIL counter: got=%d expected=%d\n", counter, expected);
        return 1;
    }
    printf("mutex_counter: ok got=%d\n", counter);

    printf("CONTRACT_PASS\n");
    return 0;
}
