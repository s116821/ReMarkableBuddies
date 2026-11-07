//! Test-only append/exclusive HTTP model used by real Worker/GoogleDrive tests.
//! It supplies no production provider qualification or configurable transport.
use super::*;
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
};

#[derive(Clone)]
struct File {
    metadata: Value,
    bytes: Vec<u8>,
}
struct Uploading {
    metadata: Value,
    total: usize,
    bytes: Vec<u8>,
}
type RequestObserver = Arc<dyn Fn(&str) + Send + Sync>;
#[derive(Default)]
struct Model {
    files: BTreeMap<String, File>,
    uploads: BTreeMap<String, Uploading>,
    next: u64,
    requests: Vec<String>,
    lose_create: Option<String>,
    observer: Option<RequestObserver>,
}
pub(in crate::storage) struct Wire {
    base: String,
    model: Arc<Mutex<Model>>,
    stop: Arc<AtomicBool>,
    server: Option<thread::JoinHandle<()>>,
}
impl Wire {
    pub fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let model = Arc::new(Mutex::new(Model::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let captured = model.clone();
        let ended = stop.clone();
        let endpoint = base.clone();
        let server = thread::spawn(move || {
            let mut children = Vec::new();
            while !ended.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let model = captured.clone();
                        let base = endpoint.clone();
                        children.push(thread::spawn(move || serve(stream, &base, &model)));
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2))
                    }
                    Err(e) => panic!("shared fixture accept: {e}"),
                }
            }
            for child in children {
                child.join().unwrap();
            }
        });
        Self {
            base,
            model,
            stop,
            server: Some(server),
        }
    }
    pub fn client(&self, credential_path: PathBuf, group: Uuid) -> GoogleDrive {
        files::directory(credential_path.parent().unwrap()).unwrap();
        files::atomic_json(
            &credential_path,
            &Credentials {
                credential_version: 1,
                client_id: "shared-wire-app".into(),
                client_secret: None,
                access_token: "shared-wire-token".into(),
                refresh_token: "shared-wire-refresh".into(),
                expires_at: now() + 3600,
                scopes: vec![APP_SCOPE.into()],
                account_permission_id: "shared-wire-account".into(),
                collection: group,
            },
        )
        .unwrap();
        let mut client = GoogleDrive::open(credential_path).unwrap();
        client.api = format!("{}/drive/v3", self.base);
        client.upload_api = format!("{}/upload/drive/v3", self.base);
        client.token_url = format!("{}/token", self.base);
        client
    }
    pub fn object(&self, id: &str, group: Uuid, bytes: Vec<u8>) {
        let tag = Tag {
            collection: group,
            kind: "object".into(),
            sha256: digest(&bytes),
        };
        self.model.lock().unwrap().files.insert(
            id.into(),
            File {
                metadata: json!({"id":id,"appProperties":tag.properties(),"trashed":false}),
                bytes,
            },
        );
    }
    pub fn slot(&self, binding: &slot::SlotBinding, id: &str, bytes: Vec<u8>) {
        self.object(id, binding.group, bytes.clone());
        self.model.lock().unwrap().files.insert(
            binding.slot_id.clone(),
            File {
                metadata: json!({"id":binding.slot_id,"trashed":false,
            "appProperties":{"buddy_protocol":"1","kind":"commit-slot","collection":binding.group,
                "descriptor_id":id,"sha256":digest(&bytes),"scope_sha256":binding.scope_sha256}}),
                bytes: vec![],
            },
        );
    }
    pub fn requests(&self) -> Vec<String> {
        self.model.lock().unwrap().requests.clone()
    }
    pub fn lose_create(&self, id: &str) {
        self.model.lock().unwrap().lose_create = Some(id.into());
    }
    pub fn observe_requests(&self, observer: impl Fn(&str) + Send + Sync + 'static) {
        self.model.lock().unwrap().observer = Some(Arc::new(observer));
    }
}
impl Drop for Wire {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.server.take().unwrap().join().unwrap();
    }
}
fn find(bytes: &[u8], needle: &[u8]) -> usize {
    bytes
        .windows(needle.len())
        .position(|w| w == needle)
        .expect("shared fixture framing")
}
fn serve(mut stream: TcpStream, base: &str, model: &Arc<Mutex<Model>>) {
    stream.set_nonblocking(false).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 8192];
    let end = loop {
        let n = stream.read(&mut buffer).unwrap();
        assert!(n > 0);
        bytes.extend_from_slice(&buffer[..n]);
        if let Some(p) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            break p + 4;
        }
        assert!(bytes.len() < 65536);
    };
    let headers = String::from_utf8(bytes[..end].to_vec()).unwrap();
    let header = |key: &str| {
        headers.lines().find_map(|line| {
            line.split_once(':')
                .filter(|(k, _)| k.eq_ignore_ascii_case(key))
                .map(|(_, v)| v.trim().to_owned())
        })
    };
    let length = header("content-length")
        .map(|s| s.parse::<usize>().unwrap())
        .unwrap_or(0);
    assert!(length <= 32 * 1024 * 1024);
    while bytes.len() < end + length {
        let n = stream.read(&mut buffer).unwrap();
        assert!(n > 0);
        bytes.extend_from_slice(&buffer[..n]);
    }
    let first = headers.lines().next().unwrap();
    let mut request = first.split_whitespace();
    let method = request.next().unwrap();
    let target = request.next().unwrap();
    let path = target.split('?').next().unwrap();
    let body = &bytes[end..end + length];
    let observer = model.lock().unwrap().observer.clone();
    if let Some(observer) = observer {
        observer(first);
    }
    let mut model = model.lock().unwrap();
    model.requests.push(first.into());
    let mut extra = Vec::<(String, String)>::new();
    let (status, response) = if method == "GET" && path == "/drive/v3/about" {
        (
            200,
            serde_json::to_vec(&json!({"user":{"permissionId":"shared-wire-account"}})).unwrap(),
        )
    } else if method == "GET" && path == "/drive/v3/files/generateIds" {
        model.next += 1;
        (
            200,
            serde_json::to_vec(&json!({"ids":[format!("generated-{}",model.next)]})).unwrap(),
        )
    } else if method == "GET" && path == "/drive/v3/files" {
        (200,serde_json::to_vec(&json!({"files":model.files.values().map(|f|f.metadata.clone()).collect::<Vec<_>>()})).unwrap())
    } else if method == "GET" && path.starts_with("/drive/v3/files/") {
        let id = path.trim_start_matches("/drive/v3/files/");
        match model.files.get(id) {
            Some(file) if target.contains("alt=media") => (200, file.bytes.clone()),
            Some(file) => (200, serde_json::to_vec(&file.metadata).unwrap()),
            None => (404, b"{}".to_vec()),
        }
    } else if method == "POST" && path == "/drive/v3/files" {
        let metadata: Value = serde_json::from_slice(body).unwrap();
        let id = metadata["id"].as_str().unwrap().to_owned();
        let status = if model.files.contains_key(&id) {
            409
        } else {
            model.files.insert(
                id.clone(),
                File {
                    metadata,
                    bytes: vec![],
                },
            );
            201
        };
        if model.lose_create.as_deref() == Some(&id) {
            model.lose_create = None;
            return;
        }
        (status, serde_json::to_vec(&json!({"id":id})).unwrap())
    } else if method == "POST"
        && path == "/upload/drive/v3/files"
        && target.contains("uploadType=multipart")
    {
        let boundary = header("content-type")
            .unwrap()
            .split("boundary=")
            .nth(1)
            .unwrap()
            .to_owned();
        let start = find(body, b"\r\n\r\n") + 4;
        let separator = format!("\r\n--{boundary}\r\n");
        let split = start + find(&body[start..], separator.as_bytes());
        let metadata: Value = serde_json::from_slice(&body[start..split]).unwrap();
        let data_start = split + separator.len();
        let data_start = data_start + find(&body[data_start..], b"\r\n\r\n") + 4;
        let ending = format!("\r\n--{boundary}--\r\n");
        assert!(body.ends_with(ending.as_bytes()));
        let data = body[data_start..body.len() - ending.len()].to_vec();
        let id = metadata["id"].as_str().unwrap().to_owned();
        let status = if model.files.contains_key(&id) {
            409
        } else {
            model.files.insert(
                id.clone(),
                File {
                    metadata,
                    bytes: data,
                },
            );
            200
        };
        (status, serde_json::to_vec(&json!({"id":id})).unwrap())
    } else if method == "POST"
        && path == "/upload/drive/v3/files"
        && target.contains("uploadType=resumable")
    {
        let metadata: Value = serde_json::from_slice(body).unwrap();
        let id = metadata["id"].as_str().unwrap().to_owned();
        if model.files.contains_key(&id) {
            (409, b"{}".to_vec())
        } else {
            let total = header("x-upload-content-length").unwrap().parse().unwrap();
            model.uploads.insert(
                id.clone(),
                Uploading {
                    metadata,
                    total,
                    bytes: vec![],
                },
            );
            extra.push((
                "Location".into(),
                format!("{base}/upload/drive/v3/session/{id}"),
            ));
            (200, b"{}".to_vec())
        }
    } else if method == "PUT" && path.starts_with("/upload/drive/v3/session/") {
        let id = path
            .trim_start_matches("/upload/drive/v3/session/")
            .to_owned();
        if let Some(upload) = model.uploads.get_mut(&id) {
            let range = header("content-range").unwrap();
            if !range.starts_with("bytes */") {
                let start = range
                    .strip_prefix("bytes ")
                    .unwrap()
                    .split('-')
                    .next()
                    .unwrap()
                    .parse::<usize>()
                    .unwrap();
                assert_eq!(start, upload.bytes.len());
                upload.bytes.extend_from_slice(body);
                assert!(upload.bytes.len() <= upload.total);
            }
            if upload.bytes.len() == upload.total {
                let upload = model.uploads.remove(&id).unwrap();
                model.files.insert(
                    id.clone(),
                    File {
                        metadata: upload.metadata,
                        bytes: upload.bytes,
                    },
                );
                (200, serde_json::to_vec(&json!({"id":id})).unwrap())
            } else {
                if !upload.bytes.is_empty() {
                    extra.push((
                        "Range".into(),
                        format!("bytes=0-{}", upload.bytes.len() - 1),
                    ));
                }
                (308, b"{}".to_vec())
            }
        } else {
            (404, b"{}".to_vec())
        }
    } else if method == "POST" && path == "/token" {
        (
            200,
            serde_json::to_vec(
                &json!({"access_token":"shared-wire-token","expires_in":3600,"scope":APP_SCOPE}),
            )
            .unwrap(),
        )
    } else {
        panic!("unexpected shared wire request: {first}")
    };
    drop(model);
    write!(
        stream,
        "HTTP/1.1 {status} Fixture\r\nContent-Length: {}\r\nConnection: close\r\n",
        response.len()
    )
    .unwrap();
    for (key, value) in extra {
        write!(stream, "{key}: {value}\r\n").unwrap();
    }
    stream.write_all(b"\r\n").unwrap();
    stream.write_all(&response).unwrap();
}

#[test]
fn download_accepts_exact_bound_and_rejects_extra_bytes() {
    let wire = Wire::new();
    let root = std::env::temp_dir().join(format!("buddy-download-bound-{}", Uuid::new_v4()));
    let group = Uuid::new_v4();
    wire.object("bounded-object", group, b"exact bytes".to_vec());
    let mut client = wire.client(root.join("drive.json"), group);
    assert_eq!(
        client.download("bounded-object", 11).unwrap(),
        b"exact bytes"
    );
    assert!(client.download("bounded-object", 10).is_err());
    assert!(client.download("bounded-object", 0).is_err());
    drop(client);
    std::fs::remove_dir_all(root).unwrap();
}
