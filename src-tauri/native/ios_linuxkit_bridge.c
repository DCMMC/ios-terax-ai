#include <pthread.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "LinuxInterop.h"

typedef void (*terax_output_cb)(void *user, const uint8_t *data, uintptr_t len);
typedef void (*terax_exit_cb)(void *user, int32_t code);
typedef void (*terax_log_cb)(const char *data, uintptr_t len);

struct fakefsify_error {
    int line;
    enum {
        ERR_ARCHIVE,
        ERR_SQLITE,
        ERR_POSIX,
        ERR_CANCELLED,
    } type;
    int code;
    char *message;
};

struct progress {
    void *cookie;
    void (*callback)(void *cookie, double progress, const char *message, bool *cancel_out);
};

extern bool fakefs_import(const char *archive_path, const char *fs, struct fakefsify_error *err_out, struct progress progress);

struct terax_terminal {
    struct linux_tty *tty;
    terax_output_cb output;
    terax_exit_cb exit;
    void *user;
    int pid;
    bool closed;
};

struct pending_terminal {
    terax_output_cb output;
    terax_exit_cb exit;
    void *user;
};

static pthread_mutex_t g_lock = PTHREAD_MUTEX_INITIALIZER;
static pthread_mutex_t g_start_lock = PTHREAD_MUTEX_INITIALIZER;
static struct pending_terminal g_pending = {0};
static char *g_root_path = NULL;
static terax_log_cb g_log = NULL;
static bool g_booted = false;

enum { TERAX_TTY_MAJOR = 4 };

static char *dup_cstr(const char *s) {
    if (!s) return NULL;
    size_t len = strlen(s);
    char *copy = malloc(len + 1);
    if (!copy) return NULL;
    memcpy(copy, s, len + 1);
    return copy;
}

void terax_linuxkit_set_log_callback(terax_log_cb cb) {
    pthread_mutex_lock(&g_lock);
    g_log = cb;
    pthread_mutex_unlock(&g_lock);
}

int32_t terax_linuxkit_import_root_tar(const char *archive_path, const char *root_path, char *error_out, uintptr_t error_len) {
    if (!archive_path || !root_path) return -22;
    struct fakefsify_error err = {0};
    bool ok = fakefs_import(archive_path, root_path, &err, (struct progress){0});
    if (ok) return 0;

    if (error_out && error_len > 0) {
        const char *message = err.message ? err.message : "fakefs import failed";
        snprintf(error_out, (size_t)error_len, "%s (type=%d code=%d line=%d)", message, err.type, err.code, err.line);
    }
    free(err.message);
    return err.code > 0 ? -err.code : -1;
}

int32_t terax_linuxkit_boot(const char *root_path) {
    pthread_mutex_lock(&g_lock);
    if (g_booted) {
        pthread_mutex_unlock(&g_lock);
        return 0;
    }
    free(g_root_path);
    g_root_path = dup_cstr(root_path);
    if (!g_root_path) {
        pthread_mutex_unlock(&g_lock);
        return -12;
    }
    g_booted = true;
    pthread_mutex_unlock(&g_lock);

    actuate_kernel("");
    return 0;
}

int32_t terax_linuxkit_start_session(
    const char *exe,
    const char *const *argv,
    const char *const *envp,
    terax_output_cb output,
    terax_exit_cb exit,
    void *user,
    void **terminal_out,
    int32_t *pid_out
) {
    if (!terminal_out || !pid_out) return -22;

    pthread_mutex_lock(&g_start_lock);
    pthread_mutex_lock(&g_lock);
    g_pending.output = output;
    g_pending.exit = exit;
    g_pending.user = user;
    pthread_mutex_unlock(&g_lock);

    __block int32_t retval = -1;
    __block int32_t pid = 0;
    __block void *terminal = NULL;

    sync_do_in_workqueue(^(void (^done)(void)) {
        linux_start_session(exe, argv, envp, ^(int rv, int child_pid, nsobj_t term) {
            retval = rv;
            pid = child_pid;
            terminal = (void *)term;
            done();
        });
    });

    if (retval < 0 || !terminal) {
        pthread_mutex_lock(&g_lock);
        memset(&g_pending, 0, sizeof(g_pending));
        pthread_mutex_unlock(&g_lock);
        pthread_mutex_unlock(&g_start_lock);
        return retval < 0 ? retval : -5;
    }

    struct terax_terminal *t = (struct terax_terminal *)terminal;
    pthread_mutex_lock(&g_lock);
    if (!t->output) {
        t->output = output;
        t->exit = exit;
        t->user = user;
    }
    memset(&g_pending, 0, sizeof(g_pending));
    pthread_mutex_unlock(&g_lock);
    t->pid = pid;
    *terminal_out = terminal;
    *pid_out = pid;
    pthread_mutex_unlock(&g_start_lock);
    return 0;
}

void terax_linuxkit_terminal_send(void *terminal, const uint8_t *data, uintptr_t len) {
    struct terax_terminal *t = (struct terax_terminal *)terminal;
    if (!t || !data || len == 0) return;
    struct linux_tty *tty = t->tty;
    if (!tty || !tty->ops || !tty->ops->send_input) return;

    char *copy = malloc((size_t)len);
    if (!copy) return;
    memcpy(copy, data, (size_t)len);
    async_do_in_workqueue(^{
        if (!t->closed && t->tty == tty) {
            tty->ops->send_input(tty, copy, (size_t)len);
        }
        free(copy);
    });
}

void terax_linuxkit_terminal_resize(void *terminal, int32_t cols, int32_t rows) {
    struct terax_terminal *t = (struct terax_terminal *)terminal;
    if (!t || cols <= 0 || rows <= 0) return;
    struct linux_tty *tty = t->tty;
    if (!tty || !tty->ops || !tty->ops->resize) return;
    async_do_in_workqueue(^{
        if (!t->closed && t->tty == tty) {
            tty->ops->resize(tty, cols, rows);
        }
    });
}

void terax_linuxkit_terminal_close(void *terminal) {
    struct terax_terminal *t = (struct terax_terminal *)terminal;
    if (!t) return;
    if (t->closed) return;
    t->closed = true;
    struct linux_tty *tty = t->tty;
    if (tty && tty->ops && tty->ops->hangup) {
        async_do_in_workqueue(^{
            if (t->tty == tty) {
                tty->ops->hangup(tty);
            }
        });
    }
    if (t->exit) t->exit(t->user, 0);
}

void async_do_in_ios(void (^block)(void)) {
    block();
}

void sync_do_in_workqueue(void (^block)(void (^done)(void))) {
    __block pthread_mutex_t mutex = PTHREAD_MUTEX_INITIALIZER;
    __block pthread_cond_t cond = PTHREAD_COND_INITIALIZER;
    __block bool flag = false;
    async_do_in_workqueue(^{
        block(^{
            pthread_mutex_lock(&mutex);
            flag = true;
            pthread_mutex_unlock(&mutex);
            pthread_cond_broadcast(&cond);
        });
    });
    pthread_mutex_lock(&mutex);
    while (!flag) {
        pthread_cond_wait(&cond, &mutex);
    }
    pthread_mutex_unlock(&mutex);
}

void ConsoleLog(const char *data, unsigned len) {
    terax_log_cb cb = NULL;
    pthread_mutex_lock(&g_lock);
    cb = g_log;
    pthread_mutex_unlock(&g_lock);
    if (cb) cb(data, len);
}

void ReportPanic(const char *message) {
    if (!message) return;
    ConsoleLog(message, (unsigned)strlen(message));
}

const char *DefaultRootPath(void) {
    return g_root_path ? g_root_path : "/";
}

nsobj_t objc_get(nsobj_t object) {
    return object;
}

void objc_put(nsobj_t object) {
    (void)object;
}

nsobj_t Terminal_terminalWithType_number(int type, int number) {
    (void)number;
    struct terax_terminal *t = calloc(1, sizeof(*t));
    if (!t) return NULL;

    if (type != TERAX_TTY_MAJOR) {
        pthread_mutex_lock(&g_lock);
        t->output = g_pending.output;
        t->exit = g_pending.exit;
        t->user = g_pending.user;
        memset(&g_pending, 0, sizeof(g_pending));
        pthread_mutex_unlock(&g_lock);
    }

    return (nsobj_t)t;
}

void Terminal_setLinuxTTY(nsobj_t self, struct linux_tty *tty) {
    struct terax_terminal *t = (struct terax_terminal *)self;
    if (!t) return;
    t->tty = tty;
    if (!tty && !t->closed) {
        t->closed = true;
        if (t->exit) t->exit(t->user, 0);
    }
}

int Terminal_sendOutput_length(nsobj_t self, const char *data, int size) {
    struct terax_terminal *t = (struct terax_terminal *)self;
    if (!t || !t->output || !data || size <= 0 || t->closed) return 0;
    t->output(t->user, (const uint8_t *)data, (uintptr_t)size);
    return size;
}

int Terminal_roomForOutput(nsobj_t self) {
    (void)self;
    return 1 << 20;
}

long UIPasteboard_changeCount(void) {
    return 0;
}

nsobj_t UIPasteboard_get(void) {
    return NULL;
}

void UIPasteboard_set(const char *data, size_t len) {
    (void)data;
    (void)len;
}

size_t NSData_length(nsobj_t data) {
    (void)data;
    return 0;
}

const void *NSData_bytes(nsobj_t data) {
    (void)data;
    return NULL;
}
