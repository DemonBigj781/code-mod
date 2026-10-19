#include <pthread.h>
#include <stdio.h>

static _Thread_local int value = 7;

static void *worker(void *unused) {
    (void)unused;
    value = 42;
    return value == 42 ? NULL : (void *)1;
}

int main(void) {
    pthread_t thread;
    void *result;
    if (pthread_create(&thread, NULL, worker, NULL) != 0 ||
        pthread_join(thread, &result) != 0 || result != NULL || value != 7) {
        return 1;
    }
    puts("Source-built fork C runtime and TLS: OK");
    return 0;
}
