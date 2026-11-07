//! Validate test intent before discovery or output writes.

use std::collections::BTreeSet;

use rebaze_cmake::{CMakeProject, CMakeTest, Executable};

#[derive(Debug, thiserror::Error)]
#[error("cannot migrate CMake tests: {0}")]
pub struct TestError(String);

#[derive(Debug)]
pub struct PlannedTest<'a> {
    pub name: String,
    pub registration: &'a CMakeTest,
    pub executable: &'a Executable,
}

pub fn plan(project: &CMakeProject) -> Result<Vec<PlannedTest<'_>>, TestError> {
    if !project.testing.tests.is_empty() && project.testing.has_launcher {
        return Err(TestError(
            "test launchers/emulators and their initializers require CMake evaluation".into(),
        ));
    }
    if !project.testing.unsupported.is_empty() {
        return Err(TestError(project.testing.unsupported.join("; ")));
    }
    if !project.testing.tests.is_empty() && !project.testing.enabled {
        return Err(TestError("tests require an unconditional root enable_testing(); include(CTest) and directory-only enabling need evaluation".into()));
    }
    let mut names: BTreeSet<_> = project
        .libraries
        .iter()
        .map(|target| &target.name)
        .chain(project.executables.iter().map(|target| &target.name))
        .map(|name| name.replace('-', "_"))
        .collect();
    project
        .testing
        .tests
        .iter()
        .map(|test| {
            if test.name.is_empty()
                || !test
                    .name
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
            {
                return Err(TestError(format!(
                    "test name {:?} must contain only ASCII letters, digits, '_' or '-'",
                    test.name
                )));
            }
            let name = format!("ctest_{}", test.name.replace('-', "_"));
            if !names.insert(name.clone()) {
                return Err(TestError(format!(
                    "test {:?} collides with generated target {name:?}",
                    test.name
                )));
            }
            let mut executables = project
                .executables
                .iter()
                .filter(|target| target.name == test.executable);
            let executable = executables.next().ok_or_else(|| {
                TestError(format!(
                    "test {:?} command {:?} is not a known executable target",
                    test.name, test.executable
                ))
            })?;
            if executables.next().is_some() {
                return Err(TestError(format!(
                    "test {:?} command is ambiguous",
                    test.name
                )));
            }
            if test
                .args
                .iter()
                .any(|arg| arg.chars().any(char::is_control))
            {
                return Err(TestError(format!(
                    "test {:?} arguments contain control characters",
                    test.name
                )));
            }
            Ok(PlannedTest {
                name,
                registration: test,
                executable,
            })
        })
        .collect()
}
