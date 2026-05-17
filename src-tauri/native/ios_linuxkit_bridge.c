#include <pthread.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <arpa/inet.h>
#include <netdb.h>
#include <netinet/in.h>
#include <resolv.h>
#include <sys/stat.h>

#include "kernel/calls.h"
#include "kernel/errno.h"
#include "kernel/fs.h"
#include "kernel/init.h"
#include "kernel/task.h"
#include "fs/dev.h"
#include "fs/devices.h"
#include "fs/fd.h"
#include "fs/path.h"
#include "fs/tty.h"

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
extern const struct fs_ops fakefs;

struct terax_terminal {
    struct tty *tty;
    terax_output_cb output;
    terax_exit_cb exit;
    void *user;
    struct task *task;
    int pid;
    bool closed;
    struct terax_terminal *next;
};

static pthread_mutex_t g_lock = PTHREAD_MUTEX_INITIALIZER;
static terax_log_cb g_log = NULL;
static bool g_booted = false;
static struct terax_terminal *g_terminals = NULL;
static struct tty *g_console_ttys[64];
static struct tty_driver g_ios_console_driver;
static struct tty_driver g_ios_pty_driver;

static void terax_log(const char *message) {
    if (!message) return;
    terax_log_cb cb = NULL;
    pthread_mutex_lock(&g_lock);
    cb = g_log;
    pthread_mutex_unlock(&g_lock);
    if (cb) {
        cb(message, (uintptr_t)strlen(message));
    } else {
        fputs(message, stderr);
        fflush(stderr);
    }
}

static char *join_root_data_path(const char *root_path) {
    if (!root_path) return NULL;
    size_t root_len = strlen(root_path);
    const char *suffix = "/data";
    size_t suffix_len = strlen(suffix);
    char *path = malloc(root_len + suffix_len + 1);
    if (!path) return NULL;
    memcpy(path, root_path, root_len);
    memcpy(path + root_len, suffix, suffix_len + 1);
    return path;
}

static int packed_count(const char *const *items) {
    int count = 0;
    if (!items) return 0;
    while (items[count]) count++;
    return count;
}

static char *pack_strings(const char *const *items) {
    size_t size = 1;
    if (items) {
        for (int i = 0; items[i]; i++) {
            size += strlen(items[i]) + 1;
        }
    }
    char *packed = calloc(1, size);
    if (!packed) return NULL;
    char *out = packed;
    if (items) {
        for (int i = 0; items[i]; i++) {
            size_t len = strlen(items[i]);
            memcpy(out, items[i], len);
            out += len + 1;
        }
    }
    *out = '\0';
    return packed;
}

static struct terax_terminal *terminal_for_tty(struct tty *tty) {
    if (!tty) return NULL;
    return (struct terax_terminal *)tty->data;
}

static void register_terminal(struct terax_terminal *terminal) {
    pthread_mutex_lock(&g_lock);
    terminal->next = g_terminals;
    g_terminals = terminal;
    pthread_mutex_unlock(&g_lock);
}

static void unregister_terminal(struct terax_terminal *terminal) {
    pthread_mutex_lock(&g_lock);
    struct terax_terminal **slot = &g_terminals;
    while (*slot) {
        if (*slot == terminal) {
            *slot = terminal->next;
            break;
        }
        slot = &(*slot)->next;
    }
    pthread_mutex_unlock(&g_lock);
}

static int terax_tty_init(struct tty *tty) {
    struct terax_terminal *terminal = calloc(1, sizeof(*terminal));
    if (!terminal) return _ENOMEM;
    terminal->tty = tty;
    tty->data = terminal;
    register_terminal(terminal);
    return 0;
}

static int terax_tty_write(struct tty *tty, const void *buf, size_t len, bool blocking) {
    (void)blocking;
    struct terax_terminal *terminal = terminal_for_tty(tty);
    if (!terminal || terminal->closed || !terminal->output || !buf || len == 0) return 0;
    terminal->output(terminal->user, (const uint8_t *)buf, (uintptr_t)len);
    return (int)len;
}

static void terax_tty_cleanup(struct tty *tty) {
    struct terax_terminal *terminal = terminal_for_tty(tty);
    if (!terminal) return;
    tty->data = NULL;
    terminal->tty = NULL;
    unregister_terminal(terminal);
}

static const struct tty_driver_ops g_ios_pty_ops = {
    .init = terax_tty_init,
    .write = terax_tty_write,
    .cleanup = terax_tty_cleanup,
};

static void terax_handle_exit(struct task *task, int code) {
    if (!task) return;
    char log_line[160];
    int parent_pid = task->parent ? task->parent->pid : 0;
    int grandparent_pid = task->parent && task->parent->parent ? task->parent->parent->pid : 0;
    snprintf(log_line, sizeof(log_line),
             "ios-linuxkit exit task pid=%d comm=%s parent=%d grandparent=%d code=%d\n",
             task->pid, task->comm, parent_pid, grandparent_pid, code);
    terax_log(log_line);

    // Match ios-linuxkit's AppDelegate.m: only init and direct children of init
    // represent top-level sessions. Foreground commands spawned by the shell
    // are grandchildren; treating those exits as terminal exits makes Terax
    // respawn the terminal after every command.
    if (task->parent != NULL && task->parent->parent != NULL) {
        terax_log("ios-linuxkit exit ignored: child process\n");
        return;
    }

    pthread_mutex_lock(&g_lock);
    for (struct terax_terminal *terminal = g_terminals; terminal; terminal = terminal->next) {
        if (terminal->task == task && !terminal->closed) {
            terminal->closed = true;
            terax_exit_cb exit = terminal->exit;
            void *user = terminal->user;
            pthread_mutex_unlock(&g_lock);
            if (exit) exit(user, code);
            return;
        }
    }
    pthread_mutex_unlock(&g_lock);
}

void terax_linuxkit_set_log_callback(terax_log_cb cb) {
    pthread_mutex_lock(&g_lock);
    g_log = cb;
    pthread_mutex_unlock(&g_lock);
}

static void terax_write_file(const char *path, const char *data) {
    if (!path || !data) return;
    current = pid_get_task(1);
    struct fd *fd = generic_open(path, O_WRONLY_ | O_CREAT_ | O_TRUNC_, 0666);
    if (IS_ERR(fd)) return;
    if (fd->ops && fd->ops->write) {
        fd->ops->write(fd, data, strlen(data));
    }
    fd_close(fd);
}

static void terax_configure_dns(void) {
    struct __res_state res;
    memset(&res, 0, sizeof(res));
    if (res_ninit(&res) != 0) {
        terax_log("ios-linuxkit dns resolver init failed\n");
        return;
    }

    char resolv_conf[2048];
    size_t used = 0;
    if (res.dnsrch[0] != NULL) {
        used += snprintf(resolv_conf + used, sizeof(resolv_conf) - used, "search");
        for (int i = 0; res.dnsrch[i] != NULL && used < sizeof(resolv_conf); i++) {
            used += snprintf(resolv_conf + used, sizeof(resolv_conf) - used, " %s", res.dnsrch[i]);
        }
        if (used < sizeof(resolv_conf)) {
            used += snprintf(resolv_conf + used, sizeof(resolv_conf) - used, "\n");
        }
    }

    union res_sockaddr_union servers[NI_MAXSERV];
    int servers_found = res_getservers(&res, servers, NI_MAXSERV);
    char address[NI_MAXHOST];
    for (int i = 0; i < servers_found && used < sizeof(resolv_conf); i++) {
        union res_sockaddr_union server = servers[i];
        if (server.sin.sin_len == 0) continue;
        int err = getnameinfo((struct sockaddr *)&server.sin, server.sin.sin_len,
                              address, sizeof(address), NULL, 0, NI_NUMERICHOST);
        if (err != 0) continue;
        used += snprintf(resolv_conf + used, sizeof(resolv_conf) - used,
                         "nameserver %s\n", address);
    }

    if (used == 0) {
        snprintf(resolv_conf, sizeof(resolv_conf), "nameserver 1.1.1.1\nnameserver 8.8.8.8\n");
    } else {
        resolv_conf[sizeof(resolv_conf) - 1] = '\0';
    }

    terax_write_file("/etc/resolv.conf", resolv_conf);
    terax_log("ios-linuxkit dns configured\n");
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
    g_booted = true;
    pthread_mutex_unlock(&g_lock);

    char *data_path = join_root_data_path(root_path);
    if (!data_path) return -12;

    int err = mount_root(&fakefs, data_path);
    free(data_path);
    if (err < 0) return err;

    err = become_first_process();
    if (err < 0) return err;

    g_ios_console_driver.ops = &g_ios_pty_ops;
    g_ios_console_driver.major = TTY_CONSOLE_MAJOR;
    g_ios_console_driver.ttys = g_console_ttys;
    g_ios_console_driver.limit = 64;
    g_ios_pty_driver.ops = &g_ios_pty_ops;
    generic_mknodat(AT_PWD, "/dev/tty1", S_IFCHR | 0666, dev_make(TTY_CONSOLE_MAJOR, 1));
    generic_mknodat(AT_PWD, "/dev/tty2", S_IFCHR | 0666, dev_make(TTY_CONSOLE_MAJOR, 2));
    generic_mknodat(AT_PWD, "/dev/tty3", S_IFCHR | 0666, dev_make(TTY_CONSOLE_MAJOR, 3));
    generic_mknodat(AT_PWD, "/dev/tty4", S_IFCHR | 0666, dev_make(TTY_CONSOLE_MAJOR, 4));
    generic_mknodat(AT_PWD, "/dev/tty5", S_IFCHR | 0666, dev_make(TTY_CONSOLE_MAJOR, 5));
    generic_mknodat(AT_PWD, "/dev/tty6", S_IFCHR | 0666, dev_make(TTY_CONSOLE_MAJOR, 6));
    generic_mknodat(AT_PWD, "/dev/tty7", S_IFCHR | 0666, dev_make(TTY_CONSOLE_MAJOR, 7));
    generic_mknodat(AT_PWD, "/dev/tty", S_IFCHR | 0666, dev_make(TTY_ALTERNATE_MAJOR, DEV_TTY_MINOR));
    generic_mknodat(AT_PWD, "/dev/console", S_IFCHR | 0666, dev_make(TTY_ALTERNATE_MAJOR, DEV_CONSOLE_MINOR));
    generic_mknodat(AT_PWD, "/dev/ptmx", S_IFCHR | 0666, dev_make(TTY_ALTERNATE_MAJOR, DEV_PTMX_MINOR));
    generic_mknodat(AT_PWD, "/dev/null", S_IFCHR | 0666, dev_make(MEM_MAJOR, DEV_NULL_MINOR));
    generic_mknodat(AT_PWD, "/dev/zero", S_IFCHR | 0666, dev_make(MEM_MAJOR, DEV_ZERO_MINOR));
    generic_mknodat(AT_PWD, "/dev/full", S_IFCHR | 0666, dev_make(MEM_MAJOR, DEV_FULL_MINOR));
    generic_mknodat(AT_PWD, "/dev/random", S_IFCHR | 0666, dev_make(MEM_MAJOR, DEV_RANDOM_MINOR));
    generic_mknodat(AT_PWD, "/dev/urandom", S_IFCHR | 0666, dev_make(MEM_MAJOR, DEV_URANDOM_MINOR));
    generic_mkdirat(AT_PWD, "/dev/pts", 0755);
    generic_mkdirat(AT_PWD, "/mnt", 0755);
    generic_setattrat(AT_PWD, "/", (struct attr){.type = attr_mode, .mode = 0755}, false);
    do_mount(&procfs, "proc", "/proc", "", 0);
    do_mount(&devptsfs, "devpts", "/dev/pts", "", 0);

    tty_drivers[TTY_CONSOLE_MAJOR] = &g_ios_console_driver;
    set_console_device(TTY_CONSOLE_MAJOR, 1);
    create_stdio("/dev/console", TTY_CONSOLE_MAJOR, 1);
    terax_configure_dns();
    exit_hook = terax_handle_exit;
    terax_log("ios-linuxkit ARM64 root booted\n");
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
    if (!exe || !terminal_out || !pid_out) return -22;

    int err = become_new_init_child();
    if (err < 0) return err;

    struct tty *tty = pty_open_fake(&g_ios_pty_driver);
    if (IS_ERR(tty)) return (int32_t)PTR_ERR(tty);
    struct terax_terminal *terminal = terminal_for_tty(tty);
    if (!terminal) return -5;
    terminal->output = output;
    terminal->exit = exit;
    terminal->user = user;

    char stdio_file[64];
    snprintf(stdio_file, sizeof(stdio_file), "/dev/pts/%d", tty->num);
    err = create_stdio(stdio_file, TTY_PSEUDO_SLAVE_MAJOR, tty->num);
    tty_release(tty);
    if (err < 0) return err;

    int argc = packed_count(argv);
    char *packed_argv = pack_strings(argv);
    char *packed_envp = pack_strings(envp);
    if (!packed_argv || !packed_envp) {
        free(packed_argv);
        free(packed_envp);
        return -12;
    }

    err = do_execve(exe, (size_t)argc, packed_argv, packed_envp);
    free(packed_argv);
    free(packed_envp);
    if (err < 0) return err;

    terminal->task = current;
    terminal->pid = current->pid;
    char log_line[160];
    snprintf(log_line, sizeof(log_line),
             "ios-linuxkit session task pid=%d comm=%s tty=%d\n",
             terminal->pid, current->comm, tty->num);
    terax_log(log_line);
    *terminal_out = terminal;
    *pid_out = terminal->pid;
    task_start(current);
    return 0;
}

void terax_linuxkit_terminal_send(void *terminal, const uint8_t *data, uintptr_t len) {
    struct terax_terminal *t = (struct terax_terminal *)terminal;
    if (!t || !t->tty || !data || len == 0 || t->closed) return;
    tty_input(t->tty, (const char *)data, (size_t)len, false);
}

void terax_linuxkit_terminal_resize(void *terminal, int32_t cols, int32_t rows) {
    struct terax_terminal *t = (struct terax_terminal *)terminal;
    if (!t || !t->tty || cols <= 0 || rows <= 0) return;
    lock(&t->tty->lock);
    tty_set_winsize(t->tty, (struct winsize_){.col = (word_t)cols, .row = (word_t)rows});
    unlock(&t->tty->lock);
}

void terax_linuxkit_terminal_close(void *terminal) {
    struct terax_terminal *t = (struct terax_terminal *)terminal;
    if (!t || t->closed) return;
    t->closed = true;
    if (t->tty) {
        lock(&t->tty->lock);
        tty_hangup(t->tty);
        unlock(&t->tty->lock);
    }
    if (t->exit) t->exit(t->user, 0);
}

void ConsoleLog(const char *data, unsigned len) {
    if (!data || len == 0) return;
    terax_log_cb cb = NULL;
    pthread_mutex_lock(&g_lock);
    cb = g_log;
    pthread_mutex_unlock(&g_lock);
    if (cb) {
        cb(data, len);
    } else {
        fwrite(data, 1, len, stderr);
        fflush(stderr);
    }
}

void ReportPanic(const char *message) {
    if (!message) return;
    ConsoleLog(message, (unsigned)strlen(message));
    ConsoleLog("\n", 1);
}
