use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use pklr::capabilities::BoxFuture;
use pklr::{Error, EvalCapabilities, Evaluator};

const MAIN: &str = "/work/config/main.pkl";
const FILE_IMPORT: &str = "/work/config/file_import.pkl";
const LOCAL_AMEND: &str = "/work/config/base.pkl";
const FILE_IMPORT_URI: &str = "file:///work/config/deps/../file_import.pkl";
const PACKAGE_URL: &str = "https://github.com/acme/pkg/releases/download/v1/Package.pkl";
const REWRITTEN_HTTP: &str = "https://packages.test/acme/download/notes.txt";

#[derive(Debug, Clone, PartialEq, Eq)]
enum TraceEvent {
    Exists(String),
    Canonicalize { input: String, output: String },
    Read(String),
    ReadBytes(String),
    MakeDir(String),
    Write(String),
    ReadEnv(String),
    FetchText(String),
    FetchBytes(String),
    Temp(String),
    Extract(String),
}

#[derive(Clone)]
struct TraceHost {
    files: Arc<Mutex<HashMap<PathBuf, Vec<u8>>>>,
    envs: Arc<Mutex<BTreeMap<String, Option<String>>>>,
    trace: Arc<Mutex<Vec<TraceEvent>>>,
}

impl TraceHost {
    fn new() -> Self {
        Self {
            files: Arc::new(Mutex::new(HashMap::new())),
            envs: Arc::new(Mutex::new(BTreeMap::new())),
            trace: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn write_file(&self, path: &str, source: impl AsRef<str>) {
        self.files
            .lock()
            .unwrap()
            .insert(PathBuf::from(path), source.as_ref().as_bytes().to_vec());
    }

    fn seed_fixtures(&self) {
        self.write_file(
            MAIN,
            format!(
                r#"amends "base.pkl"
import "{FILE_IMPORT_URI}" as FileImport
import "package://pkg.pkl-lang.org/github.com/acme/pkg@v1#/Package.pkl" as PackageModule
import "package://pkg.pkl-lang.org/github.com/acme/pkg@v1#/Package.pkl" as PackageAgain

answer = PackageModule.pkgValue + PackageAgain.pkgValue - PackageModule.pkgValue + FileImport.fileValue + 100
environment = read("env:PKLR_TRACE_PRESENT")
missingEnvironment = read?("env:PKLR_TRACE_MISSING")
httpFirst = read("https://github.com/acme/pkg/releases/download/notes.txt")
httpSecond = read("https://github.com/acme/pkg/releases/download/notes.txt")
"#,
            ),
        );
        self.write_file(FILE_IMPORT, "fileValue = 7\n");
        self.write_file(
            LOCAL_AMEND,
            "baseValue = 100\nbaseEnv = read(\"env:PKLR_TRACE_BASE\")\n",
        );

        let mut envs = self.envs.lock().unwrap();
        envs.insert(
            "PKLR_TRACE_PRESENT".to_string(),
            Some("present".to_string()),
        );
        envs.insert(
            "PKLR_TRACE_BASE".to_string(),
            Some("base-value".to_string()),
        );
        envs.insert("PKLR_TRACE_MISSING".to_string(), None);
    }

    fn events(&self) -> Vec<TraceEvent> {
        self.trace.lock().unwrap().clone()
    }

    fn normalized(&self, path: &Path) -> PathBuf {
        let mut output = PathBuf::new();
        for component in path.components() {
            use std::path::Component::*;
            match component {
                Prefix(prefix) => output.push(prefix.as_os_str()),
                RootDir => output.push("/"),
                CurDir => {}
                ParentDir => {
                    output.pop();
                }
                Normal(part) => output.push(part),
            }
        }
        output
    }

    fn lookup(&self, path: &Path) -> Option<Vec<u8>> {
        self.files
            .lock()
            .unwrap()
            .get(&self.normalized(path))
            .cloned()
    }
}

impl EvalCapabilities for TraceHost {
    fn read_to_string<'a>(&'a mut self, path: &'a Path) -> BoxFuture<'a, pklr::Result<String>> {
        self.trace
            .lock()
            .unwrap()
            .push(TraceEvent::Read(path.display().to_string()));
        let bytes = self.lookup(path);
        let path = path.to_path_buf();
        Box::pin(async move {
            let bytes = bytes.ok_or_else(|| {
                Error::Io(
                    path.clone(),
                    std::io::Error::from(std::io::ErrorKind::NotFound),
                )
            })?;
            String::from_utf8(bytes).map_err(|error| Error::Eval(error.to_string()))
        })
    }

    fn path_exists<'a>(&'a mut self, path: &'a Path) -> BoxFuture<'a, pklr::Result<bool>> {
        self.trace
            .lock()
            .unwrap()
            .push(TraceEvent::Exists(path.display().to_string()));
        let exists = self.lookup(path).is_some();
        Box::pin(async move { Ok(exists) })
    }

    fn canonicalize<'a>(&'a mut self, path: &'a Path) -> BoxFuture<'a, pklr::Result<PathBuf>> {
        let output = self.normalized(path);
        self.trace.lock().unwrap().push(TraceEvent::Canonicalize {
            input: path.display().to_string(),
            output: output.display().to_string(),
        });
        Box::pin(async move { Ok(output) })
    }

    fn read_bytes<'a>(&'a mut self, path: &'a Path) -> BoxFuture<'a, pklr::Result<Vec<u8>>> {
        self.trace
            .lock()
            .unwrap()
            .push(TraceEvent::ReadBytes(path.display().to_string()));
        let bytes = self.lookup(path);
        let path = path.to_path_buf();
        Box::pin(async move {
            bytes.ok_or_else(|| Error::Io(path, std::io::Error::from(std::io::ErrorKind::NotFound)))
        })
    }

    fn create_dir_all<'a>(&'a mut self, path: &'a Path) -> BoxFuture<'a, pklr::Result<()>> {
        self.trace
            .lock()
            .unwrap()
            .push(TraceEvent::MakeDir(path.display().to_string()));
        Box::pin(async move { Ok(()) })
    }

    fn write_atomic<'a>(
        &'a mut self,
        path: &'a Path,
        bytes: &'a [u8],
    ) -> BoxFuture<'a, pklr::Result<()>> {
        let normalized = self.normalized(path);
        self.trace
            .lock()
            .unwrap()
            .push(TraceEvent::Write(normalized.display().to_string()));
        self.files
            .lock()
            .unwrap()
            .insert(normalized, bytes.to_vec());
        Box::pin(async move { Ok(()) })
    }

    fn read_env<'a>(&'a mut self, name: &'a str) -> BoxFuture<'a, pklr::Result<Option<String>>> {
        self.trace
            .lock()
            .unwrap()
            .push(TraceEvent::ReadEnv(name.to_string()));
        let value = self.envs.lock().unwrap().get(name).cloned().flatten();
        Box::pin(async move { Ok(value) })
    }

    fn fetch_text<'a>(&'a mut self, url: &'a str) -> BoxFuture<'a, pklr::Result<String>> {
        self.trace
            .lock()
            .unwrap()
            .push(TraceEvent::FetchText(url.to_string()));
        Box::pin(async move {
            if url == REWRITTEN_HTTP {
                Ok("note".to_string())
            } else {
                Err(Error::Eval(format!("unexpected HTTP fetch: {url}")))
            }
        })
    }

    fn fetch_bytes<'a>(&'a mut self, url: &'a str) -> BoxFuture<'a, pklr::Result<Vec<u8>>> {
        self.trace
            .lock()
            .unwrap()
            .push(TraceEvent::FetchBytes(url.to_string()));
        Box::pin(async move { Err(Error::Eval(format!("unexpected package fetch: {url}"))) })
    }

    fn temp_dir<'a>(&'a mut self, prefix: &'a str) -> BoxFuture<'a, pklr::Result<PathBuf>> {
        self.trace
            .lock()
            .unwrap()
            .push(TraceEvent::Temp(prefix.to_string()));
        Box::pin(async move { Ok(PathBuf::from("/tmp/trace-package")) })
    }

    fn glob<'a>(
        &'a mut self,
        _base: &'a Path,
        _pattern: &'a str,
    ) -> BoxFuture<'a, pklr::Result<Vec<PathBuf>>> {
        Box::pin(async move { Ok(Vec::new()) })
    }

    fn extract_zip<'a>(
        &'a mut self,
        _bytes: Vec<u8>,
        destination: &'a Path,
    ) -> BoxFuture<'a, pklr::Result<()>> {
        self.trace
            .lock()
            .unwrap()
            .push(TraceEvent::Extract(destination.display().to_string()));
        Box::pin(async move { Ok(()) })
    }
}

async fn run_trace(
    offline: bool,
) -> (
    pklr::Result<(serde_json::Value, BTreeMap<String, Option<String>>)>,
    Vec<TraceEvent>,
) {
    let host = TraceHost::new();
    host.seed_fixtures();
    let mut evaluator = Evaluator::with_capabilities(host.clone());
    evaluator.set_package_cache_dir("/cache");
    evaluator.set_http_rewrites(&[
        "https://github.com/acme/=https://short.test/".to_string(),
        "https://github.com/acme/pkg/releases/download/=https://packages.test/acme/download/"
            .to_string(),
    ]);
    evaluator.set_offline(offline);
    if !offline {
        evaluator
            .preload_package_async(PACKAGE_URL, "pkl", b"pkgValue = 20\n")
            .await
            .unwrap();
    }

    let result = async {
        let value = evaluator.eval_file_pub(Path::new(MAIN)).await?;
        let value = evaluator.apply_converters(value).await?;
        let env_reads = evaluator.env_reads().clone();
        Ok((value.to_json(), env_reads))
    }
    .await;
    (result, host.events())
}

fn assert_common_trace(trace: &[TraceEvent]) {
    assert!(trace.contains(&TraceEvent::Canonicalize {
        input: "/work/config/deps/../file_import.pkl".to_string(),
        output: FILE_IMPORT.to_string(),
    }));
    assert!(trace.contains(&TraceEvent::Canonicalize {
        input: LOCAL_AMEND.to_string(),
        output: LOCAL_AMEND.to_string(),
    }));
    assert!(trace.contains(&TraceEvent::Read(
        "/work/config/deps/../file_import.pkl".to_string(),
    )));
    assert!(trace.contains(&TraceEvent::Read(LOCAL_AMEND.to_string())));

    let writes = trace
        .iter()
        .filter(|event| matches!(event, TraceEvent::Write(path) if path.starts_with("/cache/packages/")))
        .count();
    assert_eq!(writes, 2, "preload writes data and URL sidecar: {trace:#?}");

    let fetch_text = trace
        .iter()
        .filter(|event| matches!(event, TraceEvent::FetchText(_)))
        .collect::<Vec<_>>();
    assert_eq!(
        fetch_text,
        vec![&TraceEvent::FetchText(REWRITTEN_HTTP.to_string())],
        "longest rewrite wins, and the second HTTP read is an in-memory cache hit"
    );
    assert!(!trace.iter().any(|event| matches!(
        event,
        TraceEvent::FetchText(url) if url.contains("github.com") || url.contains("short.test")
    )));

    let package_data_reads = trace
        .iter()
        .filter(|event| {
            matches!(event, TraceEvent::ReadBytes(path) if path.starts_with("/cache/packages/"))
        })
        .count();
    assert!(
        package_data_reads == 1,
        "first package import is a persistent-cache hit and the second alias is an in-memory hit: {trace:#?}"
    );
    assert!(
        !trace
            .iter()
            .any(|event| matches!(event, TraceEvent::FetchBytes(_)))
    );
    assert!(
        !trace
            .iter()
            .any(|event| matches!(event, TraceEvent::Temp(_) | TraceEvent::Extract(_)))
    );

    let base_env_count = trace
        .iter()
        .filter(|event| matches!(event, TraceEvent::ReadEnv(name) if name == "PKLR_TRACE_BASE"))
        .count();
    assert_eq!(base_env_count, 11);
    assert_eq!(
        trace
            .iter()
            .filter(
                |event| matches!(event, TraceEvent::ReadEnv(name) if name == "PKLR_TRACE_PRESENT")
            )
            .count(),
        1
    );
    assert_eq!(
        trace
            .iter()
            .filter(
                |event| matches!(event, TraceEvent::ReadEnv(name) if name == "PKLR_TRACE_MISSING")
            )
            .count(),
        1
    );
}

#[tokio::test]
async fn sync_and_async_evaluation_share_observable_trace() {
    let (async_result, async_trace) = Box::pin(run_trace(false)).await;
    let (sync_result, sync_trace) = pollster::block_on(run_trace(false));
    let (async_json, async_env) = async_result.unwrap();
    let (sync_json, sync_env) = sync_result.unwrap();

    assert_eq!(async_json, sync_json);
    assert_eq!(async_json["answer"], 127);
    assert_eq!(async_json["environment"], "present");
    assert_eq!(async_json["baseEnv"], "base-value");
    assert_eq!(async_json["missingEnvironment"], serde_json::Value::Null);
    assert_eq!(async_json["httpFirst"], "note");
    assert_eq!(async_json["httpSecond"], "note");

    let expected_env = BTreeMap::from([
        (
            "PKLR_TRACE_BASE".to_string(),
            Some("base-value".to_string()),
        ),
        ("PKLR_TRACE_MISSING".to_string(), None),
        (
            "PKLR_TRACE_PRESENT".to_string(),
            Some("present".to_string()),
        ),
    ]);
    assert_eq!(async_env, expected_env);
    assert_eq!(sync_env, expected_env);

    assert_common_trace(&async_trace);
    assert_common_trace(&sync_trace);
    assert_eq!(async_trace, sync_trace);
}

#[tokio::test]
async fn offline_rejects_uncached_package_before_network() {
    let (result, trace) = Box::pin(run_trace(true)).await;
    let error = result.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("package is not cached and offline mode is enabled")
    );
    assert!(error.to_string().contains(PACKAGE_URL));
    assert!(trace.iter().any(|event| matches!(
        event,
        TraceEvent::Read(path) if path.starts_with("/cache/packages/") && path.ends_with(".url")
    )));
    assert!(!trace.iter().any(|event| matches!(
        event,
        TraceEvent::ReadBytes(path) if path.starts_with("/cache/packages/")
    )));
    assert!(
        !trace
            .iter()
            .any(|event| matches!(event, TraceEvent::FetchText(_) | TraceEvent::FetchBytes(_)))
    );
}

async fn run_circular_trace() -> (serde_json::Value, Vec<TraceEvent>) {
    let host = TraceHost::new();
    host.write_file(
        "/work/cycle/a.pkl",
        "import \"b.pkl\" as B\na = 1\nfromB = B.b\n",
    );
    host.write_file(
        "/work/cycle/b.pkl",
        "import \"a.pkl\" as A\nb = 2\nfromA = A.a\n",
    );

    let mut evaluator = Evaluator::with_capabilities(host.clone());
    let value = evaluator
        .eval_file_pub(Path::new("/work/cycle/a.pkl"))
        .await
        .unwrap();
    (value.to_json(), host.events())
}

#[test]
fn circular_local_import_returns_placeholder_with_same_path_guard() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let (async_json, async_trace) = runtime.block_on(run_circular_trace());
    let (sync_json, sync_trace) = pollster::block_on(run_circular_trace());

    assert_eq!(async_json, sync_json);
    let json = sync_json;
    assert_eq!(json["a"], 1);
    assert_eq!(json["fromB"], 2);
    assert_eq!(json["fromA"], serde_json::Value::Null);

    assert_eq!(async_trace, sync_trace);
    assert!(sync_trace.windows(2).any(|window| {
        matches!(
            &window[0],
            TraceEvent::Canonicalize { input, output } if input == "/work/cycle/b.pkl" && output == "/work/cycle/b.pkl"
        ) && matches!(
            &window[1],
            TraceEvent::Canonicalize { input, output } if input == "/work/cycle/a.pkl" && output == "/work/cycle/a.pkl"
        )
    }));
}
