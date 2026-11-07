#![allow(clippy::unwrap_used)]

use std::collections::VecDeque;
use std::io::Write;
use std::process::ExitCode;

use super::*;

type ProbeReply = (ProbeOperation, Vec<String>, Result<ProbeOutput, RunError>);

struct FakeRunner {
    replies: VecDeque<ProbeReply>,
    calls: usize,
}

impl ProbeRunner for FakeRunner {
    fn run(
        &mut self,
        operation: ProbeOperation,
        packages: &[String],
    ) -> impl std::future::Future<Output = Result<ProbeOutput, RunError>> {
        let (expected_operation, expected_packages, output) = self.replies.pop_front().unwrap();
        assert_eq!(operation, expected_operation);
        assert_eq!(packages, expected_packages);
        self.calls += 1;
        std::future::ready(output)
    }
}

fn module() -> PkgConfigModule {
    PkgConfigModule {
        prefix: "MY_LIB".into(),
        packages: vec!["my-lib>=1.2".into(), "second".into()],
        required: false,
    }
}

fn status(code: i32) -> ExitStatus {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        ExitStatus::from_raw(code << 8)
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::ExitStatusExt;
        ExitStatus::from_raw(code.cast_unsigned())
    }
}

fn output(code: i32, stdout: &str, stderr: &str) -> ProbeOutput {
    ProbeOutput {
        status: status(code),
        stdout: stdout.into(),
        stderr: stderr.into(),
    }
}

fn successful_runner(prefix: &str) -> FakeRunner {
    use ProbeOperation::{Cflags, Exists, Libs, Prefix};
    let packages = module().packages;
    FakeRunner {
        replies: [
            (Exists, packages.clone(), Ok(output(0, "", ""))),
            (Cflags, packages.clone(), Ok(output(0, "-I '/sdk with spaces/include' -I '/sdk with spaces-other/include' -I/sdk-other/include -DVALUE=1", ""))),
            (Libs, packages.clone(), Ok(output(0, "-L '/sdk with spaces/lib' -l custom -Wl,-rpath,/sdk/lib", ""))),
            (Prefix, packages[..1].to_vec(), Ok(output(0, prefix, ""))),
        ].into(), calls: 0,
    }
}

#[cfg(unix)]
#[tokio::test]
async fn requests_preserve_version_constraints_and_query_prefix_once() {
    let mut runner = successful_runner("/sdk with spaces\n");
    let snapshot = discover_with(&[module()], &mut runner).await.unwrap();
    assert_eq!(runner.calls, 4);
    assert!(runner.replies.is_empty());
    let info = snapshot.package(&module()).unwrap().unwrap();
    assert_eq!(info.cflags[0], "-I/sdk with spaces/include");
    let system = info.system.as_ref().unwrap();
    assert_eq!(system.include_dirs[0], "include");
    assert!(system.include_dirs.contains(&"/sdk-other/include".into()));
    assert_eq!(system.lib_dirs, ["lib"]);
    assert_eq!(system.libs, ["custom"]);
    assert!(info.linkopts.contains(&"-lcustom".into()));
    assert!(info.linkopts.contains(&"-Wl,-rpath,/sdk/lib".into()));
    let config = rebaze_bazel::MigrationConfig::default();
    let project = rebaze_cmake::CMakeProject {
        name: "example".into(),
        pkg_config_modules: vec![module()],
        ..Default::default()
    };
    let first =
        rebaze_bazel::generate_from_cmake_with_config(&project, &config, &snapshot).unwrap();
    assert_eq!(
        first,
        rebaze_bazel::generate_from_cmake_with_config(&project, &config, &snapshot).unwrap()
    );
    assert_eq!(runner.calls, 4);
}

#[tokio::test]
async fn unavailable_and_broken_tools_do_not_become_the_same_observation() {
    let packages = module().packages;
    for (reply, expected) in [
        (
            Err(RunError::ToolNotFound),
            ProbeOutcome::Unavailable(UnavailableReason::ToolNotFound),
        ),
        (
            Ok(output(1, "", "not installed")),
            ProbeOutcome::Unavailable(UnavailableReason::UnsatisfiedRequest {
                diagnostic: "not installed".into(),
            }),
        ),
        (
            Ok(output(2, "", "tool broken")),
            ProbeOutcome::Failed(ProbeFailure::CommandFailed {
                operation: ProbeOperation::Exists,
                code: Some(2),
                diagnostic: "tool broken".into(),
            }),
        ),
        (
            Err(RunError::Failed(ProbeFailure::Timeout)),
            ProbeOutcome::Failed(ProbeFailure::Timeout),
        ),
    ] {
        let mut runner = FakeRunner {
            replies: [(ProbeOperation::Exists, packages.clone(), reply)].into(),
            calls: 0,
        };
        let snapshot = discover_with(&[module()], &mut runner).await.unwrap();
        assert_eq!(snapshot.outcome(&module()).unwrap(), &expected);
        assert_eq!(runner.calls, 1);
    }
}

#[tokio::test]
async fn failed_queries_stop_discovery_and_reject_generation() {
    for (index, operation) in [
        (1, ProbeOperation::Cflags),
        (2, ProbeOperation::Libs),
        (3, ProbeOperation::Prefix),
    ] {
        let mut runner = successful_runner("/sdk");
        runner.replies[index].2 = Ok(output(3, "partial", "bad package metadata"));
        let snapshot = discover_with(&[module()], &mut runner).await.unwrap();
        assert!(
            matches!(snapshot.outcome(&module()).unwrap(), ProbeOutcome::Failed(ProbeFailure::CommandFailed { operation: actual, code: Some(3), .. }) if *actual == operation)
        );
        assert!(snapshot.package(&module()).is_err());
        assert_eq!(runner.calls, index + 1);
    }
}

#[cfg(unix)]
#[tokio::test]
async fn invalid_flag_quoting_and_prefix_fail_explicitly() {
    let mut runner = successful_runner("/sdk");
    runner.replies[1].2 = Ok(output(0, "-I'unterminated", ""));
    let snapshot = discover_with(&[module()], &mut runner).await.unwrap();
    assert_eq!(
        snapshot.outcome(&module()).unwrap(),
        &ProbeOutcome::Failed(ProbeFailure::InvalidFlags)
    );
    for prefix in ["relative", "/sdk\n/other"] {
        let snapshot = discover_with(&[module()], &mut successful_runner(prefix))
            .await
            .unwrap();
        assert_eq!(
            snapshot.outcome(&module()).unwrap(),
            &ProbeOutcome::Failed(ProbeFailure::InvalidPrefix)
        );
    }
    let snapshot = discover_with(&[module()], &mut successful_runner(""))
        .await
        .unwrap();
    assert!(
        snapshot
            .package(&module())
            .unwrap()
            .unwrap()
            .system
            .is_none()
    );
}

#[tokio::test]
async fn invalid_requests_never_reach_the_process_runner() {
    for packages in [
        vec![],
        vec![String::new()],
        vec![" --define-variable=x=y".into()],
        vec!["bad\npackage".into()],
        vec!["x".repeat(64 * 1024 + 1)],
    ] {
        let request = PkgConfigModule {
            packages,
            ..module()
        };
        let mut runner = FakeRunner {
            replies: VecDeque::new(),
            calls: 0,
        };
        let snapshot = discover_with(std::slice::from_ref(&request), &mut runner)
            .await
            .unwrap();
        assert!(matches!(
            snapshot.outcome(&request).unwrap(),
            ProbeOutcome::Failed(ProbeFailure::InvalidRequest(_))
        ));
        assert_eq!(runner.calls, 0);
    }
}

#[cfg(unix)]
#[test]
fn path_relativization_respects_component_boundaries() {
    assert_eq!(relative_path("/sdk/include", "/sdk"), "include");
    assert_eq!(
        relative_path("/sdk-other/include", "/sdk"),
        "/sdk-other/include"
    );
    assert_eq!(relative_path("/sdk/include", "/"), "sdk/include");
}

#[test]
fn flag_parsing_accepts_escaped_spaces_and_rejects_missing_arguments() {
    assert_eq!(
        parse_flags("-I/sdk\\ with\\ spaces/include -L '/sdk/lib dir' -lfoo")
            .ok()
            .unwrap(),
        ["-I/sdk with spaces/include", "-L/sdk/lib dir", "-lfoo"]
    );
    for value in ["-I", "-L ''", "-l -L/sdk"] {
        assert!(matches!(
            parse_flags(value),
            Err(ProbeError::Failed(ProbeFailure::InvalidFlags))
        ));
    }
}

#[test]
fn empty_discovery_and_invalid_limits_do_not_spawn_a_tool() {
    let mut options = DiscoveryOptions {
        program: "rebaze-absent-pkg-config".into(),
        ..Default::default()
    };
    assert_eq!(
        discover_packages(&[], &options).unwrap(),
        DiscoverySnapshot::default()
    );
    for limit in [0, 16 * 1024 * 1024 + 1] {
        options.max_output_bytes = limit;
        assert!(matches!(
            discover_packages(&[module()], &options),
            Err(DiscoveryError::InvalidLimits)
        ));
    }
    options.max_output_bytes = 1024;
    options.command_timeout = Duration::ZERO;
    assert!(matches!(
        discover_packages(&[module()], &options),
        Err(DiscoveryError::InvalidLimits)
    ));
}

#[tokio::test]
async fn synchronous_discovery_is_safe_inside_an_existing_runtime() {
    let options = DiscoveryOptions {
        program: std::env::temp_dir().join("rebaze-nonexistent-directory/pkg-config"),
        ..Default::default()
    };
    let snapshot = discover_packages(&[module()], &options).unwrap();
    assert_eq!(
        snapshot.outcome(&module()).unwrap(),
        &ProbeOutcome::Unavailable(UnavailableReason::ToolNotFound)
    );
}

fn helper(mode: &str, bytes: usize) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "discovery::tests::process_helper",
            "--ignored",
            "--nocapture",
        ])
        .env("REBAZE_DISCOVERY_HELPER_MODE", mode)
        .env("REBAZE_DISCOVERY_HELPER_BYTES", bytes.to_string());
    command
}

// Only explicitly selected child test processes run this fixture. No installed
// package tool or shell is needed, and the parent owns timeout/termination.
#[test]
#[ignore = "fixture for discovery child-process tests"]
fn process_helper() -> ExitCode {
    let mode = std::env::var("REBAZE_DISCOVERY_HELPER_MODE").unwrap();
    let bytes: usize = std::env::var("REBAZE_DISCOVERY_HELPER_BYTES")
        .unwrap()
        .parse()
        .unwrap();
    match mode.as_str() {
        "stdout" => std::io::stdout().write_all(&vec![b'x'; bytes]).unwrap(),
        "stderr" => std::io::stderr().write_all(&vec![b'x'; bytes]).unwrap(),
        "invalid-utf8" => std::io::stdout().write_all(&[0xff, 0xfe]).unwrap(),
        "invalid-utf8-stderr" => std::io::stderr().write_all(&[0xff, 0xfe]).unwrap(),
        "sleep" => std::thread::sleep(Duration::from_secs(10)),
        _ => panic!("unknown helper mode"),
    }
    ExitCode::SUCCESS
}

#[tokio::test]
async fn pipe_capture_limits_both_streams_and_accepts_large_legal_output() {
    let options = DiscoveryOptions {
        command_timeout: Duration::from_secs(10),
        max_output_bytes: 256 * 1024,
        ..Default::default()
    };
    for (mode, stream) in [
        ("stdout", ProbeStream::Stdout),
        ("stderr", ProbeStream::Stderr),
    ] {
        assert!(
            matches!(run_command(helper(mode, options.max_output_bytes + 1), &options).await,
            Err(RunError::Failed(ProbeFailure::OutputLimit { stream: actual, .. })) if actual == stream)
        );
        let legal = run_command(helper(mode, 128 * 1024), &options)
            .await
            .ok()
            .unwrap();
        assert!(legal.status.success());
        assert!(
            if mode == "stdout" {
                legal.stdout.len()
            } else {
                legal.stderr.len()
            } >= 128 * 1024
        );
    }
    // Exact stderr boundary is independent of the test harness's stdout banner.
    assert!(
        run_command(helper("stderr", options.max_output_bytes), &options)
            .await
            .ok()
            .unwrap()
            .status
            .success()
    );
}

#[tokio::test]
async fn hanging_process_and_invalid_utf8_are_rejected() {
    let options = DiscoveryOptions {
        command_timeout: Duration::from_millis(200),
        ..Default::default()
    };
    let start = std::time::Instant::now();
    assert!(matches!(
        run_command(helper("sleep", 0), &options).await,
        Err(RunError::Failed(ProbeFailure::Timeout))
    ));
    assert!(start.elapsed() < Duration::from_secs(5));
    let options = DiscoveryOptions {
        command_timeout: Duration::from_secs(10),
        ..Default::default()
    };
    assert!(matches!(
        run_command(helper("invalid-utf8", 0), &options).await,
        Err(RunError::Failed(ProbeFailure::InvalidUtf8(
            ProbeStream::Stdout
        )))
    ));
}

#[tokio::test]
async fn tool_disappearing_after_availability_check_is_a_failure() {
    let mut runner = successful_runner("/sdk");
    runner.replies[1].2 = Err(RunError::ToolNotFound);
    let snapshot = discover_with(&[module()], &mut runner).await.unwrap();
    assert!(matches!(
        snapshot.outcome(&module()).unwrap(),
        ProbeOutcome::Failed(ProbeFailure::Io(_))
    ));
}

#[cfg(windows)]
#[test]
fn windows_path_relativization_respects_component_boundaries() {
    assert_eq!(relative_path(r"C:\sdk\include", r"C:\sdk"), "include");
    assert_eq!(
        relative_path(r"C:\sdk-other\include", r"C:\sdk"),
        r"C:\sdk-other\include"
    );
}

#[cfg(windows)]
#[tokio::test]
async fn windows_prefix_can_be_discovered() {
    let snapshot = discover_with(&[module()], &mut successful_runner(r"C:\sdk"))
        .await
        .unwrap();
    let info = snapshot.package(&module()).unwrap().unwrap();
    assert_eq!(info.system.as_ref().unwrap().prefix, r"C:\sdk");
}

#[cfg(unix)]
#[tokio::test]
async fn aliases_for_one_request_share_one_observation() {
    let first = module();
    let second = PkgConfigModule {
        prefix: "SECOND_ALIAS".into(),
        ..module()
    };
    let mut runner = successful_runner("/sdk with spaces");
    let snapshot = discover_with(&[first.clone(), second.clone()], &mut runner)
        .await
        .unwrap();
    assert_eq!(runner.calls, 4);
    assert_eq!(
        snapshot.outcome(&first).unwrap(),
        snapshot.outcome(&second).unwrap()
    );
}

#[tokio::test]
async fn byte_limit_accepts_exact_stdout_and_rejects_one_extra_byte() {
    let limit = 4096;
    let source = std::io::Cursor::new(vec![b'x'; limit]);
    assert_eq!(
        read_bounded(source, ProbeStream::Stdout, limit)
            .await
            .unwrap()
            .len(),
        limit
    );
    let source = std::io::Cursor::new(vec![b'x'; limit + 1]);
    assert_eq!(
        read_bounded(source, ProbeStream::Stdout, limit)
            .await
            .unwrap_err(),
        ProbeFailure::OutputLimit {
            stream: ProbeStream::Stdout,
            limit
        }
    );
}

#[tokio::test]
async fn legitimate_request_at_argument_budget_passes() {
    let request = PkgConfigModule {
        packages: vec!["x".repeat(64 * 1024)],
        ..module()
    };
    let prefix = if cfg!(windows) { r"C:\sdk" } else { "/sdk" };
    let mut runner = successful_runner(prefix);
    for reply in &mut runner.replies {
        reply.1.clone_from(&request.packages);
    }
    let snapshot = discover_with(std::slice::from_ref(&request), &mut runner)
        .await
        .unwrap();
    assert!(snapshot.package(&request).unwrap().is_some());
    assert_eq!(runner.calls, 4);
}

#[tokio::test]
async fn invalid_stderr_is_rejected_before_status_is_interpreted() {
    let options = DiscoveryOptions {
        command_timeout: Duration::from_secs(10),
        ..Default::default()
    };
    assert!(matches!(
        run_command(helper("invalid-utf8-stderr", 0), &options).await,
        Err(RunError::Failed(ProbeFailure::InvalidUtf8(
            ProbeStream::Stderr
        )))
    ));
}

#[test]
fn excessive_time_limit_is_rejected_but_maximum_limits_are_valid() {
    let mut options = DiscoveryOptions {
        command_timeout: Duration::from_secs(301),
        ..Default::default()
    };
    assert!(matches!(
        discover_packages(&[], &options),
        Err(DiscoveryError::InvalidLimits)
    ));
    options.command_timeout = Duration::from_secs(300);
    options.max_output_bytes = 16 * 1024 * 1024;
    assert_eq!(
        discover_packages(&[], &options).unwrap(),
        DiscoverySnapshot::default()
    );
}
