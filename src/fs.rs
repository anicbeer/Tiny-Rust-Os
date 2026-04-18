use spin::Mutex;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use alloc::string::{String, ToString};

pub enum FileContents {
    Static(&'static [u8]),
    Dynamic(Vec<u8>),
}

pub struct File {
    pub data: FileContents,
}

impl File {
    pub fn len(&self) -> usize {
        match &self.data {
            FileContents::Static(s) => s.len(),
            FileContents::Dynamic(v) => v.len(),
        }
    }
    pub fn as_slice(&self) -> &[u8] {
        match &self.data {
            FileContents::Static(s) => s,
            FileContents::Dynamic(v) => v.as_slice(),
        }
    }
    pub fn as_mut_vec(&mut self) -> Option<&mut Vec<u8>> {
        match &mut self.data {
            FileContents::Static(_) => None,
            FileContents::Dynamic(v) => Some(v),
        }
    }
}

#[derive(Clone, Copy)]
pub struct OpenFile {
    pub inode: usize,
    pub offset: usize,
    pub readable: bool,
    pub writable: bool,
}

static FS: Mutex<BTreeMap<String, File>> = Mutex::new(BTreeMap::new());
static NEXT_INODE: Mutex<usize> = Mutex::new(1);
static INODE_MAP: Mutex<BTreeMap<usize, String>> = Mutex::new(BTreeMap::new());

pub fn init() {
    mkdir("/etc");
    mkdir("/etc/nginx");
    mkdir("/var");
    mkdir("/var/log");
    mkdir("/var/log/nginx");
    mkdir("/usr");
    mkdir("/usr/share");
    mkdir("/usr/share/nginx");
    mkdir("/usr/share/nginx/html");
    mkdir("/usr/lib");
    mkdir("/usr/lib/riscv64-linux-gnu");
    mkdir("/usr/lib/riscv64-linux-gnu/tls");
    mkdir("/usr/lib/tls");
    mkdir("/lib");
    mkdir("/lib/riscv64-linux-gnu");
    mkdir("/lib/riscv64-linux-gnu/tls");
    mkdir("/lib/tls");
    mkdir("/tmp");
    mkdir("/tmp/nginx_install");
    mkdir("/tmp/nginx_install/logs");
    mkdir("/tmp/nginx_install/conf");
    mkdir("/tmp/nginx_install/html");
    mkdir("/sbin");
    mkdir("/sys");
    mkdir("/sys/devices");
    mkdir("/sys/devices/system");
    mkdir("/sys/devices/system/cpu");
    mkdir("/proc");

    let nginx_bin = include_bytes!("/tmp/nginx-1.26.3/objs/nginx");
    create_file_static("/sbin/nginx", nginx_bin);

    let test_bin = include_bytes!("/tmp/test_malloc2");
    create_file_static("/test_malloc", test_bin);

    let config = br#"
daemon off;
worker_processes 1;
error_log /tmp/nginx_install/logs/error.log;
pid /tmp/nginx_install/logs/nginx.pid;
events {
    worker_connections 1024;
}
http {
    server {
        listen 80;
        connection_pool_size 1024;
        server_name localhost;
        location / {
            root /tmp/nginx_install/html;
            index index.html;
        }
    }
}
"#;
    create_file_static("/etc/nginx/nginx.conf", config);
    create_file_static("/tmp/nginx_install/conf/nginx.conf", config);

    let html = b"<html><body><h1>Hello from RVOS nginx!</h1></body></html>";
    create_file_static("/usr/share/nginx/html/index.html", html);
    create_file_static("/tmp/nginx_install/html/index.html", html);

    create_file_dynamic("/var/log/nginx/error.log", b"");
    create_file_dynamic("/var/log/nginx/access.log", b"");
    create_file_dynamic("/tmp/nginx_install/logs/error.log", b"");
    create_file_dynamic("/tmp/nginx_install/logs/access.log", b"");

    mkdir("/usr/local");
    mkdir("/usr/local/nginx");
    mkdir("/usr/local/nginx/conf");
    mkdir("/usr/local/nginx/logs");
    mkdir("/usr/local/nginx/html");

    let local_config = br#"
daemon off;
master_process off;
worker_processes 1;
error_log /usr/local/nginx/logs/error.log;
pid /usr/local/nginx/logs/nginx.pid;
events {
    worker_connections 1024;
}
http {
    server {
        listen 80;
        connection_pool_size 1024;
        server_name localhost;
        location / {
            root /usr/local/nginx/html;
            index index.html;
        }
    }
}
"#;
    create_file_static("/usr/local/nginx/conf/nginx.conf", local_config);
    create_file_dynamic("/usr/local/nginx/logs/error.log", b"");
    create_file_dynamic("/usr/local/nginx/logs/access.log", b"");
    create_file_dynamic("/usr/local/nginx/logs/nginx.pid", b"");
    create_file_static("/usr/local/nginx/html/index.html", html);

    create_file_static("/sys/devices/system/cpu/online", b"0-0\n");
    create_file_static("/proc/stat", b"cpu  0 0 0 0 0 0 0 0 0 0\n");
    create_file_static("/proc/cpuinfo", b"processor\t: 0\n");
    create_file_static("/etc/localtime", b"");

    create_file_static("/etc/nsswitch.conf", b"passwd: files\n");
    create_file_static("/etc/passwd", b"root:x:0:0:root:/root:/bin/sh\n");
    create_file_static("/etc/group", b"root:x:0:\n");

    let nss_so = include_bytes!("../libnss_files.so.2");
    create_file_static("/lib/riscv64-linux-gnu/libnss_files.so.2", nss_so);
    create_file_static("/lib/riscv64-linux-gnu/tls/libnss_files.so.2", nss_so);
    create_file_static("/usr/lib/riscv64-linux-gnu/libnss_files.so.2", nss_so);
    create_file_static("/usr/lib/riscv64-linux-gnu/tls/libnss_files.so.2", nss_so);
    create_file_static("/lib/libnss_files.so.2", nss_so);
    create_file_static("/lib/tls/libnss_files.so.2", nss_so);
    create_file_static("/usr/lib/libnss_files.so.2", nss_so);
    create_file_static("/usr/lib/tls/libnss_files.so.2", nss_so);

    log::info!("RAMFS initialized with {} files", FS.lock().len());
}

pub fn mkdir(path: &str) {
    create_file_static(path, b"");
}

fn create_file_static(path: &str, data: &'static [u8]) {
    let mut fs = FS.lock();
    let mut inode_map = INODE_MAP.lock();
    let mut next_inode = NEXT_INODE.lock();
    let inode = *next_inode;
    *next_inode += 1;
    fs.insert(path.to_string(), File { data: FileContents::Static(data) });
    inode_map.insert(inode, path.to_string());
}

fn create_file_dynamic(path: &str, data: &[u8]) {
    let mut fs = FS.lock();
    let mut inode_map = INODE_MAP.lock();
    let mut next_inode = NEXT_INODE.lock();
    let inode = *next_inode;
    *next_inode += 1;
    fs.insert(path.to_string(), File { data: FileContents::Dynamic(data.to_vec()) });
    inode_map.insert(inode, path.to_string());
}

pub fn lookup(path: &str) -> Option<usize> {
    let inode_map = INODE_MAP.lock();
    for (inode, p) in inode_map.iter() {
        if p == path {
            return Some(*inode);
        }
    }
    None
}

pub fn read_inode(inode: usize, buf: &mut [u8], offset: usize) -> usize {
    let inode_map = INODE_MAP.lock();
    if let Some(path) = inode_map.get(&inode) {
        let fs = FS.lock();
        if let Some(file) = fs.get(path) {
            let data = file.as_slice();
            let len = data.len();
            let to_read = (len - offset.min(len)).min(buf.len());
            buf[..to_read].copy_from_slice(&data[offset..offset + to_read]);
            return to_read;
        }
    }
    0
}

pub fn write_inode(inode: usize, buf: &[u8], offset: usize) -> usize {
    let inode_map = INODE_MAP.lock();
    if let Some(path) = inode_map.get(&inode) {
        let mut fs = FS.lock();
        if let Some(file) = fs.get_mut(path) {
            if let Some(vec) = file.as_mut_vec() {
                if offset + buf.len() > vec.len() {
                    vec.resize(offset + buf.len(), 0);
                }
                vec[offset..offset + buf.len()].copy_from_slice(buf);
                return buf.len();
            }
        }
    }
    0
}

pub fn file_size(inode: usize) -> usize {
    let inode_map = INODE_MAP.lock();
    if let Some(path) = inode_map.get(&inode) {
        let fs = FS.lock();
        if let Some(file) = fs.get(path) {
            return file.len();
        }
    }
    0
}

pub fn is_dir(path: &str) -> bool {
    let fs = FS.lock();
    fs.get(path).map(|f| f.len() == 0).unwrap_or(false)
}

pub fn get_file_data(path: &str) -> Option<&'static [u8]> {
    // Only works for files created with create_file_static
    let fs = FS.lock();
    fs.get(path).and_then(|f| match f.data {
        FileContents::Static(s) => Some(s),
        FileContents::Dynamic(_) => None,
    })
}
