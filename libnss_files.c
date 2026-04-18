enum nss_status {
    NSS_STATUS_TRYAGAIN = -2,
    NSS_STATUS_UNAVAIL = -1,
    NSS_STATUS_NOTFOUND = 0,
    NSS_STATUS_SUCCESS = 1,
    NSS_STATUS_RETURN = 2
};

struct passwd {
    char *pw_name;
    char *pw_passwd;
    unsigned int pw_uid;
    unsigned int pw_gid;
    char *pw_gecos;
    char *pw_dir;
    char *pw_shell;
};

struct group {
    char *gr_name;
    char *gr_passwd;
    unsigned int gr_gid;
    char **gr_mem;
};

static void my_memcpy(char *dst, const char *src, unsigned long n) {
    for (unsigned long i = 0; i < n; i++) dst[i] = src[i];
}

static enum nss_status fill_root(struct passwd *result, char *buffer, unsigned long buflen, int *errnop) {
    const char *name = "root";
    const char *pass = "x";
    const char *gecos = "root";
    const char *dir = "/root";
    const char *shell = "/bin/sh";
    if (buflen < 64) {
        *errnop = 34; // ERANGE
        return NSS_STATUS_TRYAGAIN;
    }
    char *p = buffer;
    result->pw_name = p;
    my_memcpy(p, name, 5); p += 5; *p++ = 0;
    result->pw_passwd = p;
    my_memcpy(p, pass, 2); p += 2; *p++ = 0;
    result->pw_gecos = p;
    my_memcpy(p, gecos, 5); p += 5; *p++ = 0;
    result->pw_dir = p;
    my_memcpy(p, dir, 6); p += 6; *p++ = 0;
    result->pw_shell = p;
    my_memcpy(p, shell, 8); p += 8; *p++ = 0;
    result->pw_uid = 0;
    result->pw_gid = 0;
    return NSS_STATUS_SUCCESS;
}

enum nss_status _nss_files_getpwuid_r(unsigned int uid, struct passwd *result,
                                       char *buffer, unsigned long buflen, int *errnop) {
    if (uid == 0) {
        return fill_root(result, buffer, buflen, errnop);
    }
    return NSS_STATUS_NOTFOUND;
}

enum nss_status _nss_files_getpwnam_r(const char *name, struct passwd *result,
                                       char *buffer, unsigned long buflen, int *errnop) {
    if (name[0] == 'r' && name[1] == 'o' && name[2] == 'o' && name[3] == 't' && name[4] == 0) {
        return fill_root(result, buffer, buflen, errnop);
    }
    return NSS_STATUS_NOTFOUND;
}

enum nss_status _nss_files_setpwent(void) { return NSS_STATUS_SUCCESS; }
enum nss_status _nss_files_endpwent(void) { return NSS_STATUS_SUCCESS; }

enum nss_status _nss_files_getpwent_r(struct passwd *result, char *buffer, unsigned long buflen, int *errnop) {
    (void)result; (void)buffer; (void)buflen; (void)errnop;
    return NSS_STATUS_NOTFOUND;
}

static enum nss_status fill_root_group(struct group *result, char *buffer, unsigned long buflen, int *errnop) {
    const char *name = "root";
    const char *pass = "x";
    if (buflen < 32) {
        *errnop = 34;
        return NSS_STATUS_TRYAGAIN;
    }
    char *p = buffer;
    result->gr_name = p;
    my_memcpy(p, name, 5); p += 5; *p++ = 0;
    result->gr_passwd = p;
    my_memcpy(p, pass, 2); p += 2; *p++ = 0;
    result->gr_gid = 0;
    result->gr_mem = (char **)p;
    *(char **)p = 0; // NULL terminated list
    return NSS_STATUS_SUCCESS;
}

enum nss_status _nss_files_getgrgid_r(unsigned int gid, struct group *result,
                                       char *buffer, unsigned long buflen, int *errnop) {
    if (gid == 0) {
        return fill_root_group(result, buffer, buflen, errnop);
    }
    return NSS_STATUS_NOTFOUND;
}

enum nss_status _nss_files_getgrnam_r(const char *name, struct group *result,
                                       char *buffer, unsigned long buflen, int *errnop) {
    if (name[0] == 'r' && name[1] == 'o' && name[2] == 'o' && name[3] == 't' && name[4] == 0) {
        return fill_root_group(result, buffer, buflen, errnop);
    }
    return NSS_STATUS_NOTFOUND;
}

enum nss_status _nss_files_setgrent(void) { return NSS_STATUS_SUCCESS; }
enum nss_status _nss_files_endgrent(void) { return NSS_STATUS_SUCCESS; }

enum nss_status _nss_files_getgrent_r(struct group *result, char *buffer, unsigned long buflen, int *errnop) {
    (void)result; (void)buffer; (void)buflen; (void)errnop;
    return NSS_STATUS_NOTFOUND;
}
