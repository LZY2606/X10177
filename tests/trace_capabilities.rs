use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use pklr::Evaluator;
use pklr::capabilities::{BoxFuture, EvalCapabilities};

#[derive(Clone, Debug, PartialEq, Eq)]
enum TraceEvent {
    Canonicalize(String, String),
    ReadText(String),
    PathExists(String),
    ReadEnv(String),
    CacheLookup(String),
    CacheHit(String),
    CacheMiss(String),
    CacheDataRead(String),
    CacheWrite(String),
    FetchBytes(String),
}

#[derive(Clone, Default)]
struct TraceState {
    events: Arc<Mutex<Vec<TraceEvent>>>,
    files: Arc<Mutex<HashMap<String, String>>>,
    env: Arc<Mutex<HashMap<String, String>>>,
    package_bytes: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    cache: Arc<Mutex<HashMap<PathBuf, Vec<u8>>>>,
}

impl TraceState {
    fn event(&self, event: TraceEvent) {
        self.events.lock().unwrap().push(event);
    }

    fn events(&self) -> Vec<TraceEvent> {
        self.events.lock().unwrap().clone()
    }
}

struct TraceCapabilities {
    state: TraceState,
}

impl TraceCapabilities {
    fn new(state: TraceState) -> Self {
        Self { state }
    }
}

impl EvalCapabilities for TraceCapabilities {
    fn read_to_string<'a>(&'a mut self, path: &'a Path) -> BoxFuture<'a, pklr::Result<String>> {
        let key = path.to_string_lossy().into_owned();
        let state = self.state.clone();
        Box::pin(async move {
            state.event(TraceEvent::ReadText(key.clone()));
            if key.ends_with(".url") {
                let cache_path = PathBuf::from(&key);
                state.event(TraceEvent::CacheLookup(key.clone()));
                if state.cache.lock().unwrap().contains_key(&cache_path) {
                    state.event(TraceEvent::CacheHit(key.clone()));
                    return String::from_utf8(
                        state
                            .cache
                            .lock()
                            .unwrap()
                            .get(&cache_path)
                            .cloned()
                            .unwrap(),
                    )
                    .map_err(|error| pklr::Error::Eval(error.to_string()));
                } else {
                    state.event(TraceEvent::CacheMiss(key.clone()));
                    return Err(pklr::Error::Io(
                        PathBuf::from(key),
                        std::io::ErrorKind::NotFound.into(),
                    ));
                }
            }
            let contents = state
                .files
                .lock()
                .unwrap()
                .get(&key)
                .cloned()
                .ok_or_else(|| pklr::Error::ImportNotFound(key.clone()));
            contents
        })
    }

    fn path_exists<'a>(&'a mut self, path: &'a Path) -> BoxFuture<'a, pklr::Result<bool>> {
        let key = path.to_string_lossy().into_owned();
        let state = self.state.clone();
        Box::pin(async move {
            state.event(TraceEvent::PathExists(key.clone()));
            Ok(state.files.lock().unwrap().contains_key(&key))
        })
    }

    fn canonicalize<'a>(&'a mut self, path: &'a Path) -> BoxFuture<'a, pklr::Result<PathBuf>> {
        let requested = path.to_path_buf();
        let state = self.state.clone();
        Box::pin(async move {
            let normalized = match requested.to_string_lossy().as_ref() {
                "virtual/entry.pkl" => PathBuf::from("virtual/normalized/entry.pkl"),
                "virtual/Imported.pkl" => PathBuf::from("virtual/normalized/Imported.pkl"),
                "virtual/Base.pkl" => PathBuf::from("virtual/normalized/Base.pkl"),
                "virtual/a.pkl" => PathBuf::from("virtual/normalized/a.pkl"),
                "virtual/b.pkl" => PathBuf::from("virtual/normalized/b.pkl"),
                _ => requested.clone(),
            };
            state.event(TraceEvent::Canonicalize(
                requested.display().to_string(),
                normalized.display().to_string(),
            ));
            Ok(normalized)
        })
    }

    fn read_bytes<'a>(&'a mut self, path: &'a Path) -> BoxFuture<'a, pklr::Result<Vec<u8>>> {
        let path = path.to_path_buf();
        let state = self.state.clone();
        Box::pin(async move {
            state.event(TraceEvent::CacheDataRead(path.display().to_string()));
            state
                .cache
                .lock()
                .unwrap()
                .get(&path)
                .cloned()
                .ok_or_else(|| pklr::Error::Io(path.clone(), std::io::ErrorKind::NotFound.into()))
        })
    }

    fn create_dir_all<'a>(&'a mut self, path: &'a Path) -> BoxFuture<'a, pklr::Result<()>> {
        let _ = path;
        Box::pin(async { Ok(()) })
    }

    fn write_atomic<'a>(
        &'a mut self,
        path: &'a Path,
        bytes: &'a [u8],
    ) -> BoxFuture<'a, pklr::Result<()>> {
        let path = path.to_path_buf();
        let bytes = bytes.to_vec();
        let state = self.state.clone();
        Box::pin(async move {
            state.event(TraceEvent::CacheWrite(path.display().to_string()));
            state.cache.lock().unwrap().insert(path, bytes);
            Ok(())
        })
    }

    fn read_env<'a>(&'a mut self, name: &'a str) -> BoxFuture<'a, pklr::Result<Option<String>>> {
        let name = name.to_string();
        let state = self.state.clone();
        Box::pin(async move {
            state.event(TraceEvent::ReadEnv(name.clone()));
            Ok(state.env.lock().unwrap().get(&name).cloned())
        })
    }

    fn fetch_text<'a>(&'a mut self, url: &'a str) -> BoxFuture<'a, pklr::Result<String>> {
        let url = url.to_string();
        Box::pin(async move { Err(pklr::Error::Unsupported(url)) })
    }

    fn fetch_bytes<'a>(&'a mut self, url: &'a str) -> BoxFuture<'a, pklr::Result<Vec<u8>>> {
        let url = url.to_string();
        let state = self.state.clone();
        Box::pin(async move {
            state.event(TraceEvent::FetchBytes(url.clone()));
            state
                .package_bytes
                .lock()
                .unwrap()
                .get(&url)
                .cloned()
                .ok_or_else(|| pklr::Error::ImportNotFound(url))
        })
    }

    fn temp_dir<'a>(&'a mut self, prefix: &'a str) -> BoxFuture<'a, pklr::Result<PathBuf>> {
        let prefix = prefix.to_string();
        Box::pin(async move { Ok(PathBuf::from(prefix)) })
    }

    fn glob<'a>(
        &'a mut self,
        _base: &'a Path,
        _pattern: &'a str,
    ) -> BoxFuture<'a, pklr::Result<Vec<PathBuf>>> {
        Box::pin(async { Ok(Vec::new()) })
    }
}

fn trace_state() -> TraceState {
    let state = TraceState::default();
    state.files.lock().unwrap().extend(HashMap::from([
        ("virtual/entry.pkl".to_string(), ENTRY.to_string()),
        (
            "virtual/Imported.pkl".to_string(),
            IMPORTED_FILE.to_string(),
        ),
        ("virtual/Base.pkl".to_string(), AMENDS_BASE.to_string()),
        ("virtual/a.pkl".to_string(), CYCLE_A.to_string()),
        ("virtual/b.pkl".to_string(), CYCLE_B.to_string()),
    ]));
    state.env.lock().unwrap().extend(HashMap::from([
        ("PKLR_TRACE_VALUE".to_string(), "env-value".to_string()),
        ("PKLR_TRACE_FLAG".to_string(), "on".to_string()),
    ]));
    state.package_bytes.lock().unwrap().insert(
        "https://mirror.example/special/Pkg.pkl".to_string(),
        PACKAGE_SOURCE.as_bytes().to_vec(),
    );
    state
}

fn evaluator(state: TraceState, offline: bool) -> Evaluator {
    let mut evaluator = Evaluator::with_capabilities(TraceCapabilities::new(state.clone()));
    evaluator.set_base_path(Path::new("virtual"));
    evaluator.set_package_cache_dir("virtual-cache");
    evaluator.set_offline(offline);
    evaluator.set_http_rewrites(&[
        "https://github.com/acme/pkg/releases/download/=https://mirror.example/".to_string(),
        "https://github.com/acme/pkg/releases/download/v1/=https://mirror.example/special/"
            .to_string(),
    ]);
    evaluator
}

const ENTRY: &str = r#"
amends "Base.pkl"
import "file://virtual/Imported.pkl" as Imported
import "package://pkg.pkl-lang.org/github.com/acme/pkg@v1#/Pkg.pkl" as Pkg
fileValue = Imported.value
packageValue = Pkg.value
environment = read("env:PKLR_TRACE_VALUE")
optionalFlag = read?("env:PKLR_TRACE_MISSING")
presentFlag = read?("env:PKLR_TRACE_FLAG")
"#;

const IMPORTED_FILE: &str = r#"
value = "file-value"
"#;

const AMENDS_BASE: &str = "baseValue = 41\n";

const PACKAGE_SOURCE: &str = "value = \"package-value\"\n";

const CYCLE_A: &str = r#"
import "b.pkl" as b
aValue = "a"
bValue = b.bValue
"#;

const CYCLE_B: &str = r#"
import "a.pkl" as a
bValue = "b"
aValue = a.aValue
"#;

const PACKAGE_URL: &str = "https://github.com/acme/pkg/releases/download/v1/Pkg.pkl";
const FETCHED_PACKAGE_URL: &str = "https://mirror.example/special/Pkg.pkl";

fn event_label(event: &TraceEvent) -> &'static str {
    match event {
        TraceEvent::Canonicalize(path, _) if path.ends_with("entry.pkl") => "canonicalize-entry",
        TraceEvent::Canonicalize(path, _) if path.ends_with("Imported.pkl") => {
            "canonicalize-import"
        }
        TraceEvent::Canonicalize(path, _) if path.ends_with("Base.pkl") => "canonicalize-base",
        TraceEvent::Canonicalize(_, _) => "canonicalize-other",
        TraceEvent::ReadText(path) if path.ends_with("entry.pkl") => "read-entry",
        TraceEvent::ReadText(path) if path.ends_with("Imported.pkl") => "read-import",
        TraceEvent::ReadText(path) if path.ends_with("Base.pkl") => "read-base",
        TraceEvent::ReadText(path) if path.ends_with(".url") => "read-cache-url",
        TraceEvent::ReadText(_) => "read-other",
        TraceEvent::PathExists(path) if path.ends_with("Base.pkl") => "exists-base",
        TraceEvent::PathExists(path) if path.ends_with("Imported.pkl") => "exists-import",
        TraceEvent::PathExists(_) => "exists-other",
        TraceEvent::ReadEnv(name) if name == "PKLR_TRACE_VALUE" => "env-value",
        TraceEvent::ReadEnv(name) if name == "PKLR_TRACE_MISSING" => "env-missing",
        TraceEvent::ReadEnv(_) => "env-other",
        TraceEvent::CacheLookup(_) => "cache-lookup",
        TraceEvent::CacheHit(_) => "cache-hit",
        TraceEvent::CacheMiss(_) => "cache-miss",
        TraceEvent::CacheDataRead(_) => "cache-data-read",
        TraceEvent::CacheWrite(path) if path.ends_with(".pkl") => "cache-write-data",
        TraceEvent::CacheWrite(_) => "cache-write-url",
        TraceEvent::FetchBytes(_) => "fetch-bytes",
    }
}

fn assert_cold_trace_order(events: &[TraceEvent]) {
    let labels = events.iter().map(event_label).collect::<Vec<_>>();
    assert_eq!(
        labels,
        vec![
            "read-entry",
            "canonicalize-entry",
            "canonicalize-entry",
            "exists-base",
            "read-base",
            "exists-import",
            "canonicalize-base",
            "canonicalize-import",
            "canonicalize-import",
            "read-import",
            "canonicalize-import",
            "canonicalize-import",
            "read-cache-url",
            "cache-lookup",
            "cache-miss",
            "fetch-bytes",
            "cache-write-data",
            "cache-write-url",
            "exists-base",
            "canonicalize-base",
            "read-base",
            "canonicalize-base",
            "exists-base",
            "read-base",
            "canonicalize-base",
            "env-value",
            "env-missing",
            "env-other",
            "canonicalize-entry",
        ]
    );
}

async fn run_entry(
    state: TraceState,
    offline: bool,
) -> (serde_json::Value, BTreeMap<String, Option<String>>) {
    let mut evaluator = evaluator(state, offline);
    let value = evaluator
        .eval_file_pub(Path::new("virtual/entry.pkl"))
        .await
        .unwrap()
        .to_json();
    (value, evaluator.env_reads().clone())
}

fn run_entry_blocking(
    state: TraceState,
    offline: bool,
) -> (serde_json::Value, BTreeMap<String, Option<String>>) {
    pollster::block_on(run_entry(state, offline))
}

async fn run_cycle(state: TraceState) -> serde_json::Value {
    evaluator(state, false)
        .eval_file_pub(Path::new("virtual/a.pkl"))
        .await
        .unwrap()
        .to_json()
}

fn run_cycle_blocking(state: TraceState) -> serde_json::Value {
    pollster::block_on(run_cycle(state))
}

#[tokio::test]
async fn sync_and_async_traces_share_the_same_evaluation_boundaries() {
    let async_state = trace_state();
    let (async_value, async_env) = run_entry(async_state.clone(), false).await;
    let sync_state = trace_state();
    let (sync_value, sync_env) = run_entry_blocking(sync_state.clone(), false);

    assert_eq!(sync_value, async_value);
    assert_eq!(sync_env, async_env);
    assert_eq!(
        sync_value,
        serde_json::json!({
            "baseValue": 41,
            "fileValue": "file-value",
            "packageValue": "package-value",
            "environment": "env-value",
            "optionalFlag": null,
            "presentFlag": "on",
        })
    );
    assert_eq!(sync_state.events(), async_state.events());
    assert_cold_trace_order(&async_state.events());

    let events = async_state.events();
    let canonicalizations = events
        .iter()
        .filter_map(|event| match event {
            TraceEvent::Canonicalize(requested, normalized) => {
                Some((requested.as_str(), normalized.as_str()))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(canonicalizations.contains(&("virtual/entry.pkl", "virtual/normalized/entry.pkl")));
    assert!(
        canonicalizations.contains(&("virtual/Imported.pkl", "virtual/normalized/Imported.pkl"))
    );
    assert!(canonicalizations.contains(&("virtual/Base.pkl", "virtual/normalized/Base.pkl")));
    assert_eq!(
        events
            .iter()
            .filter(
                |event| matches!(event, TraceEvent::FetchBytes(url) if url == FETCHED_PACKAGE_URL)
            )
            .count(),
        1
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, TraceEvent::FetchBytes(url) if url == PACKAGE_URL))
    );
    assert_eq!(
        events
            .iter()
            .filter_map(|event| match event {
                TraceEvent::ReadEnv(name) => Some(name.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>(),
        vec!["PKLR_TRACE_VALUE", "PKLR_TRACE_MISSING", "PKLR_TRACE_FLAG"]
    );
    assert_eq!(
        async_env,
        BTreeMap::from([
            ("PKLR_TRACE_FLAG".to_string(), Some("on".to_string())),
            ("PKLR_TRACE_MISSING".to_string(), None),
            (
                "PKLR_TRACE_VALUE".to_string(),
                Some("env-value".to_string())
            ),
        ])
    );
    let async_preload_state = trace_state();
    let mut async_preloader = evaluator(async_preload_state.clone(), false);
    let package_bytes = PACKAGE_SOURCE.as_bytes();
    async_preloader
        .preload_package_async(PACKAGE_URL, "pkl", package_bytes)
        .await
        .unwrap();
    let (async_warm_value, _) = run_entry(async_preload_state.clone(), true).await;

    let sync_preload_state = trace_state();
    evaluator(sync_preload_state.clone(), false)
        .preload_package(PACKAGE_URL, "pkl", package_bytes)
        .unwrap();
    let (sync_warm_value, _) = run_entry_blocking(sync_preload_state.clone(), true);

    assert_eq!(sync_warm_value, async_warm_value);
    assert_eq!(async_warm_value["packageValue"], "package-value");
    assert_eq!(sync_preload_state.events(), async_preload_state.events());
    assert!(
        async_preload_state.events().iter().any(|event| {
            matches!(event, TraceEvent::CacheWrite(path) if path.ends_with(".pkl"))
        })
    );
    assert!(
        async_preload_state.events().iter().any(|event| {
            matches!(event, TraceEvent::CacheWrite(path) if path.ends_with(".url"))
        })
    );
    assert!(
        async_preload_state
            .events()
            .iter()
            .any(|event| { matches!(event, TraceEvent::CacheHit(_)) })
    );
    assert!(
        !async_preload_state
            .events()
            .iter()
            .any(|event| matches!(event, TraceEvent::FetchBytes(_)))
    );

    let cold_state = trace_state();
    let mut cold_evaluator = evaluator(cold_state.clone(), true);
    let cold_error = cold_evaluator
        .eval_file_pub(Path::new("virtual/entry.pkl"))
        .await
        .unwrap_err()
        .to_string();
    assert!(cold_error.contains("package is not cached and offline mode is enabled"));
    assert!(cold_error.contains(PACKAGE_URL));
    assert!(
        !cold_state
            .events()
            .iter()
            .any(|event| matches!(event, TraceEvent::FetchBytes(_)))
    );

    let mut cycle_evaluator = evaluator(trace_state(), false);
    let cycle_value = cycle_evaluator
        .eval_file_pub(Path::new("virtual/a.pkl"))
        .await
        .unwrap()
        .to_json();
    assert_eq!(cycle_value["aValue"], "a");
    assert_eq!(cycle_value["bValue"], "b");

    let async_cycle_state = trace_state();
    let async_cycle_value = run_cycle(async_cycle_state.clone()).await;
    let sync_cycle_state = trace_state();
    let sync_cycle_value = run_cycle_blocking(sync_cycle_state.clone());
    assert_eq!(sync_cycle_value, async_cycle_value);
    assert_eq!(sync_cycle_state.events(), async_cycle_state.events());
}
