#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/file.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <unistd.h>

#ifndef PAIR_MEDIA
#define PAIR_MEDIA "/mnt/us"
#endif
#ifndef PAIR_TMP
#define PAIR_TMP "/var/tmp"
#endif
#ifndef RENAME_NOREPLACE
#define RENAME_NOREPLACE 1U
#endif

enum { lockfd = 9 };
struct context { char stage[PATH_MAX], home[PATH_MAX], a[PATH_MAX], b[PATH_MAX], selector[PATH_MAX]; };

static int fail(const char *msg) { fprintf(stderr, "pair-fs: %s\n", msg); return 1; }
static int path(char *buf, const char *root, const char *tail) {
    int n = snprintf(buf,PATH_MAX,"%s/%s",root,tail); return n > 0 && n < PATH_MAX;
}
static int context(const char *owner, struct context *c) {
    const char prefix[] = PAIR_TMP "/bilicomics-";
    char name[64]; const char *id;
    if (strncmp(owner,prefix,sizeof(prefix)-1)) return 0;
    id=owner+sizeof(prefix)-1;
    if (strlen(id)!=20 || strncmp(id,"lib-p-",6) || strspn(id+6,"0123456789abcdef")!=8 || strcmp(id+14,"/owner")) return 0;
    int n=snprintf(name,sizeof(name),"bilicomics-%.14s",id);
    if (n<=0 || (size_t)n>=sizeof(name)) return 0;
    return path(c->stage,PAIR_MEDIA,name) && path(c->home,PAIR_MEDIA,"bilicomics") && path(c->a,c->stage,"park-A") &&
        path(c->b,c->stage,"park-B") && path(c->selector,c->stage,"tools/select-arms.sh");
}
static int regular_owner(const struct stat *st) { return S_ISREG(st->st_mode) && st->st_uid==geteuid() && st->st_nlink==1; }
static int held(const char *owner) {
    struct stat expected,actual;
    return lstat(owner,&expected)==0 && regular_owner(&expected) && fstat(lockfd,&actual)==0 && regular_owner(&actual) &&
        expected.st_dev==actual.st_dev && expected.st_ino==actual.st_ino && flock(lockfd,LOCK_EX|LOCK_NB)==0;
}
static int lock_exec(const char *owner, char *selector, char *arm, char *mode) {
    int fd=open(owner,O_RDONLY|O_NOFOLLOW|O_NONBLOCK); struct stat st;
    if (fd<0 || fstat(fd,&st) || !regular_owner(&st)) return fail("invalid owner inode");
    if (flock(fd,LOCK_EX|LOCK_NB)) return fail("selector already active or flock unavailable");
    if (fd!=lockfd) { if (dup2(fd,lockfd)<0) return fail("cannot carry lease descriptor"); close(fd); }
    if (fcntl(lockfd,F_SETFD,0) || !held(owner)) return fail("lock binding changed");
    char *args[]={"sh", selector, "locked", arm, mode, NULL};
    execv("/bin/sh",args); return fail("cannot execute selector");
}
static int noreplace(const char *src, const char *dst) {
#ifdef SYS_renameat2
    return (int)syscall(SYS_renameat2,AT_FDCWD,src,AT_FDCWD,dst,RENAME_NOREPLACE);
#else
    (void)src; (void)dst; errno=ENOSYS; return -1;
#endif
}
static int same_dir(const char *path_name, const struct stat *before) {
    struct stat st;
    return lstat(path_name,&st)==0 && S_ISDIR(st.st_mode) && before->st_dev==st.st_dev && before->st_ino==st.st_ino;
}
static int probe(const char *stage) {
    char root[PATH_MAX],a[PATH_MAX],b[PATH_MAX],moved[PATH_MAX]; struct stat sa,sb; int ok=0,cleanup=1;
    if (!path(root,stage,".pair-capability.XXXXXX") || !mkdtemp(root)) return fail("cannot allocate private capability probe");
    if (!path(a,root,"left") || !path(b,root,"right") || !path(moved,root,"moved")) return fail("probe path too long");
    if (!mkdir(a,0700) && !mkdir(b,0700) && !lstat(a,&sa) && !lstat(b,&sb) &&
        noreplace(a,b)==-1 && errno==EEXIST && same_dir(a,&sa) && same_dir(b,&sb) &&
        !noreplace(a,moved) && !noreplace(moved,a)) ok=1;
    /* Only these newly allocated, empty, uniquely named probe directories can be removed. */
    if (rmdir(a) && errno!=ENOENT) cleanup=0;
    if (rmdir(b) && errno!=ENOENT) cleanup=0;
    if (rmdir(moved) && errno!=ENOENT) cleanup=0;
    if (rmdir(root)) cleanup=0;
    return ok && cleanup ? 0 : fail("no-replace directory rename is not qualified on this volume");
}
int main(int argc, char **argv) {
    struct context c; struct stat st;
    if (argc<3 || !context(argv[2],&c)) return fail("unbound run");
    if (argc==6 && !strcmp(argv[1],"lock")) {
        if (strcmp(argv[3],c.selector) || (strcmp(argv[4],"A") && strcmp(argv[4],"B")) ||
            (strcmp(argv[5],"select") && strcmp(argv[5],"restore") && strcmp(argv[5],"probe"))) return fail("invalid selector action");
        return lock_exec(argv[2],argv[3],argv[4],argv[5]);
    }
    if (!held(argv[2])) return fail("original owner descriptor not locked");
    if (argc==3 && !strcmp(argv[1],"held")) return 0;
    if (argc==4 && !strcmp(argv[1],"probe") && !strcmp(argv[3],c.stage)) return probe(c.stage);
    if (argc==5 && !strcmp(argv[1],"move")) {
        const char *src=argv[3], *dst=argv[4];
        if (!((!strcmp(src,c.home) && (!strcmp(dst,c.a) || !strcmp(dst,c.b))) ||
              (!strcmp(dst,c.home) && (!strcmp(src,c.a) || !strcmp(src,c.b)))) ||
            lstat(src,&st) || !S_ISDIR(st.st_mode)) return fail("unauthorized directory move");
        if (noreplace(src,dst)) return fail("destination occupied or directory rename unavailable");
        return 0;
    }
    return fail("invalid primitive");
}
