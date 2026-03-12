//! Mock container runtime for unit testing
//!
//! Provides a configurable test double for `ContainerRuntime` that records
//! calls and returns queued or default responses.

use crate::error::{MinoError, MinoResult};
use crate::orchestration::podman::ContainerConfig;
use crate::orchestration::runtime::{ContainerRuntime, VolumeInfo};
use crate::session::{Session, SessionStatus};
use async_trait::async_trait;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// A queued response for a mock method call.
#[allow(dead_code)]
pub enum MockResponse {
    Unit,
    Bool(bool),
    String(String),
    Int(i32),
    OptionalInt(Option<i32>),
    VolumeInfoVec(Vec<VolumeInfo>),
    OptionalVolumeInfo(Option<VolumeInfo>),
    DiskUsageMap(HashMap<String, u64>),
    StringVec(Vec<String>),
}

/// Recorded method call with arguments.
#[derive(Debug, Clone)]
pub struct CallRecord {
    pub method: String,
    pub args: Vec<String>,
}

/// Mock implementation of `ContainerRuntime` for testing.
///
/// Supports queued per-method responses (FIFO) with sensible defaults,
/// and records all calls for assertion.
pub struct MockRuntime {
    responses: Mutex<HashMap<String, Vec<MinoResult<MockResponse>>>>,
    pub calls: Mutex<Vec<CallRecord>>,
}

impl MockRuntime {
    pub fn new() -> Self {
        Self {
            responses: Mutex::new(HashMap::new()),
            calls: Mutex::new(Vec::new()),
        }
    }

    /// Queue a response for a method. Responses are consumed FIFO.
    pub fn on(self, method: &str, response: MinoResult<MockResponse>) -> Self {
        self.responses
            .lock()
            .unwrap()
            .entry(method.to_string())
            .or_default()
            .push(response);
        self
    }

    /// Queue an `Ok(Unit)` response for a method.
    #[allow(dead_code)]
    pub fn on_ok(self, method: &str) -> Self {
        self.on(method, Ok(MockResponse::Unit))
    }

    /// Queue an error response for a method.
    #[allow(dead_code)]
    pub fn on_err(self, method: &str, err: MinoError) -> Self {
        self.on(method, Err(err))
    }

    /// Assert a method was called exactly `count` times.
    pub fn assert_called(&self, method: &str, count: usize) {
        let calls = self.calls.lock().unwrap();
        let actual = calls.iter().filter(|c| c.method == method).count();
        assert_eq!(
            actual, count,
            "expected {} call(s) to '{}', got {}",
            count, method, actual
        );
    }

    /// Assert a method was called with specific arguments (at least once).
    pub fn assert_called_with(&self, method: &str, expected_args: &[&str]) {
        let calls = self.calls.lock().unwrap();
        let expected: Vec<String> = expected_args.iter().map(|s| s.to_string()).collect();
        let found = calls
            .iter()
            .any(|c| c.method == method && c.args == expected);
        assert!(
            found,
            "expected call to '{}' with args {:?}, calls: {:?}",
            method,
            expected_args,
            calls
                .iter()
                .filter(|c| c.method == method)
                .collect::<Vec<_>>()
        );
    }

    /// Assert no calls were made to the runtime.
    #[allow(dead_code)]
    pub fn assert_no_calls(&self) {
        let calls = self.calls.lock().unwrap();
        assert!(calls.is_empty(), "expected no calls, got: {:?}", *calls);
    }

    fn record(&self, method: &str, args: Vec<String>) {
        self.calls.lock().unwrap().push(CallRecord {
            method: method.to_string(),
            args,
        });
    }

    fn take_response(&self, method: &str) -> Option<MinoResult<MockResponse>> {
        let mut responses = self.responses.lock().unwrap();
        let queue = responses.get_mut(method)?;
        if queue.is_empty() {
            None
        } else {
            Some(queue.remove(0))
        }
    }

    fn take_unit(&self, method: &str) -> MinoResult<()> {
        match self.take_response(method) {
            Some(Ok(MockResponse::Unit)) | None => Ok(()),
            Some(Err(e)) => Err(e),
            Some(Ok(_)) => panic!("wrong MockResponse variant for '{}'", method),
        }
    }

    fn take_bool(&self, method: &str, default: bool) -> MinoResult<bool> {
        match self.take_response(method) {
            Some(Ok(MockResponse::Bool(b))) => Ok(b),
            None => Ok(default),
            Some(Err(e)) => Err(e),
            Some(Ok(_)) => panic!("wrong MockResponse variant for '{}'", method),
        }
    }

    fn take_string(&self, method: &str, default: &str) -> MinoResult<String> {
        match self.take_response(method) {
            Some(Ok(MockResponse::String(s))) => Ok(s),
            None => Ok(default.to_string()),
            Some(Err(e)) => Err(e),
            Some(Ok(_)) => panic!("wrong MockResponse variant for '{}'", method),
        }
    }

    fn take_int(&self, method: &str, default: i32) -> MinoResult<i32> {
        match self.take_response(method) {
            Some(Ok(MockResponse::Int(i))) => Ok(i),
            None => Ok(default),
            Some(Err(e)) => Err(e),
            Some(Ok(_)) => panic!("wrong MockResponse variant for '{}'", method),
        }
    }

    fn take_optional_int(&self, method: &str, default: Option<i32>) -> MinoResult<Option<i32>> {
        match self.take_response(method) {
            Some(Ok(MockResponse::OptionalInt(i))) => Ok(i),
            None => Ok(default),
            Some(Err(e)) => Err(e),
            Some(Ok(_)) => panic!("wrong MockResponse variant for '{}'", method),
        }
    }

    fn take_volume_info_vec(&self, method: &str) -> MinoResult<Vec<VolumeInfo>> {
        match self.take_response(method) {
            Some(Ok(MockResponse::VolumeInfoVec(v))) => Ok(v),
            None => Ok(vec![]),
            Some(Err(e)) => Err(e),
            Some(Ok(_)) => panic!("wrong MockResponse variant for '{}'", method),
        }
    }

    fn take_optional_volume_info(&self, method: &str) -> MinoResult<Option<VolumeInfo>> {
        match self.take_response(method) {
            Some(Ok(MockResponse::OptionalVolumeInfo(v))) => Ok(v),
            None => Ok(None),
            Some(Err(e)) => Err(e),
            Some(Ok(_)) => panic!("wrong MockResponse variant for '{}'", method),
        }
    }

    fn take_disk_usage_map(&self, method: &str) -> MinoResult<HashMap<String, u64>> {
        match self.take_response(method) {
            Some(Ok(MockResponse::DiskUsageMap(m))) => Ok(m),
            None => Ok(HashMap::new()),
            Some(Err(e)) => Err(e),
            Some(Ok(_)) => panic!("wrong MockResponse variant for '{}'", method),
        }
    }

    fn take_string_vec(&self, method: &str) -> MinoResult<Vec<String>> {
        match self.take_response(method) {
            Some(Ok(MockResponse::StringVec(v))) => Ok(v),
            None => Ok(vec![]),
            Some(Err(e)) => Err(e),
            Some(Ok(_)) => panic!("wrong MockResponse variant for '{}'", method),
        }
    }
}

#[async_trait]
impl ContainerRuntime for MockRuntime {
    async fn is_available(&self) -> MinoResult<bool> {
        self.record("is_available", vec![]);
        self.take_bool("is_available", true)
    }

    async fn ensure_ready(&self) -> MinoResult<()> {
        self.record("ensure_ready", vec![]);
        self.take_unit("ensure_ready")
    }

    async fn run(&self, _config: &ContainerConfig, _command: &[String]) -> MinoResult<String> {
        self.record("run", vec![]);
        self.take_string("run", "mock-container-id")
    }

    async fn create(&self, _config: &ContainerConfig, _command: &[String]) -> MinoResult<String> {
        self.record("create", vec![]);
        self.take_string("create", "mock-container-id")
    }

    async fn start_attached(&self, container_id: &str) -> MinoResult<i32> {
        self.record("start_attached", vec![container_id.to_string()]);
        self.take_int("start_attached", 0)
    }

    async fn stop(&self, container_id: &str) -> MinoResult<()> {
        self.record("stop", vec![container_id.to_string()]);
        self.take_unit("stop")
    }

    async fn kill(&self, container_id: &str) -> MinoResult<()> {
        self.record("kill", vec![container_id.to_string()]);
        self.take_unit("kill")
    }

    async fn remove(&self, container_id: &str) -> MinoResult<()> {
        self.record("remove", vec![container_id.to_string()]);
        self.take_unit("remove")
    }

    async fn container_prune(&self) -> MinoResult<()> {
        self.record("container_prune", vec![]);
        self.take_unit("container_prune")
    }

    async fn logs(&self, container_id: &str, lines: u32) -> MinoResult<String> {
        self.record("logs", vec![container_id.to_string(), lines.to_string()]);
        self.take_string("logs", "")
    }

    async fn logs_follow(&self, container_id: &str) -> MinoResult<()> {
        self.record("logs_follow", vec![container_id.to_string()]);
        self.take_unit("logs_follow")
    }

    async fn image_exists(&self, image: &str) -> MinoResult<bool> {
        self.record("image_exists", vec![image.to_string()]);
        self.take_bool("image_exists", false)
    }

    async fn build_image(&self, _context_dir: &Path, tag: &str) -> MinoResult<()> {
        self.record("build_image", vec![tag.to_string()]);
        self.take_unit("build_image")
    }

    async fn build_image_with_progress(
        &self,
        _context_dir: &Path,
        tag: &str,
        on_output: &(dyn Fn(String) + Send + Sync),
    ) -> MinoResult<()> {
        self.record("build_image_with_progress", vec![tag.to_string()]);
        on_output("STEP 1: mock build".to_string());
        self.take_unit("build_image_with_progress")
    }

    async fn image_remove(&self, image: &str) -> MinoResult<()> {
        self.record("image_remove", vec![image.to_string()]);
        self.take_unit("image_remove")
    }

    async fn image_list_prefixed(&self, prefix: &str) -> MinoResult<Vec<String>> {
        self.record("image_list_prefixed", vec![prefix.to_string()]);
        self.take_string_vec("image_list_prefixed")
    }

    fn runtime_name(&self) -> &'static str {
        "mock"
    }

    async fn volume_create(&self, name: &str, _labels: &HashMap<String, String>) -> MinoResult<()> {
        self.record("volume_create", vec![name.to_string()]);
        self.take_unit("volume_create")
    }

    async fn volume_remove(&self, name: &str) -> MinoResult<()> {
        self.record("volume_remove", vec![name.to_string()]);
        self.take_unit("volume_remove")
    }

    async fn volume_list(&self, prefix: &str) -> MinoResult<Vec<VolumeInfo>> {
        self.record("volume_list", vec![prefix.to_string()]);
        self.take_volume_info_vec("volume_list")
    }

    async fn volume_inspect(&self, name: &str) -> MinoResult<Option<VolumeInfo>> {
        self.record("volume_inspect", vec![name.to_string()]);
        self.take_optional_volume_info("volume_inspect")
    }

    async fn volume_disk_usage(&self, prefix: &str) -> MinoResult<HashMap<String, u64>> {
        self.record("volume_disk_usage", vec![prefix.to_string()]);
        self.take_disk_usage_map("volume_disk_usage")
    }

    async fn get_container_exit_code(&self, container_id: &str) -> MinoResult<Option<i32>> {
        self.record("get_container_exit_code", vec![container_id.to_string()]);
        self.take_optional_int("get_container_exit_code", Some(0))
    }
}

/// Create a test session with the given name, status, and optional container ID.
pub fn test_session(name: &str, status: SessionStatus, container_id: Option<&str>) -> Session {
    let mut session = Session::new(
        name.to_string(),
        PathBuf::from("/test/project"),
        vec!["bash".to_string()],
        status,
    );
    session.container_id = container_id.map(String::from);
    session
}

/// Create a minimal `ContainerConfig` suitable for tests.
pub fn test_container_config() -> ContainerConfig {
    ContainerConfig {
        image: "test-image:latest".to_string(),
        workdir: "/workspace".to_string(),
        volumes: vec![],
        env: HashMap::new(),
        network: "bridge".to_string(),
        interactive: true,
        tty: true,
        cap_add: vec![],
        cap_drop: vec![],
        security_opt: vec![],
        pids_limit: 0,
        auto_remove: false,
        read_only: false,
        tmpfs: vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_default_responses() {
        let mock = MockRuntime::new();
        assert!(mock.is_available().await.unwrap());
        assert_eq!(mock.runtime_name(), "mock");
        assert_eq!(
            mock.create(&test_container_config(), &[]).await.unwrap(),
            "mock-container-id"
        );
        assert_eq!(mock.start_attached("abc").await.unwrap(), 0);
        assert_eq!(mock.logs("abc", 100).await.unwrap(), "");
        assert!(!mock.image_exists("img").await.unwrap());
        assert!(mock.volume_list("pfx").await.unwrap().is_empty());
        assert!(mock.volume_inspect("vol").await.unwrap().is_none());
        assert!(mock.volume_disk_usage("pfx").await.unwrap().is_empty());
        assert_eq!(mock.get_container_exit_code("abc").await.unwrap(), Some(0));
    }

    #[tokio::test]
    async fn mock_queued_responses() {
        let mock = MockRuntime::new()
            .on("logs", Ok(MockResponse::String("line1\nline2".to_string())))
            .on("logs", Ok(MockResponse::String("line3".to_string())));

        // First call returns first queued response
        assert_eq!(mock.logs("abc", 50).await.unwrap(), "line1\nline2");
        // Second call returns second queued response
        assert_eq!(mock.logs("abc", 50).await.unwrap(), "line3");
        // Third call falls back to default (empty string)
        assert_eq!(mock.logs("abc", 50).await.unwrap(), "");
    }

    #[tokio::test]
    async fn mock_records_calls() {
        let mock = MockRuntime::new();

        mock.stop("container-1").await.unwrap();
        mock.kill("container-2").await.unwrap();
        mock.remove("container-1").await.unwrap();

        mock.assert_called("stop", 1);
        mock.assert_called("kill", 1);
        mock.assert_called("remove", 1);
        mock.assert_called_with("stop", &["container-1"]);
        mock.assert_called_with("kill", &["container-2"]);
    }
}
