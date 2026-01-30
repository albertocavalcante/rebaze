//! Package and pkg-config dependency extraction.
//!
//! Handles extraction of find_package() and pkg_check_modules() commands.

use super::super::types::{CMakeProject, Package, PkgConfigModule};
use crate::ast::{Argument, Command};
use crate::eval::EvalContext;

/// Extract find_package() command.
pub fn extract_package(cmd: &Command, project: &mut CMakeProject) {
    // find_package(PackageName [version] [REQUIRED] [COMPONENTS comp1...])
    let name = if let Some(name) = cmd.arguments.first().and_then(Argument::as_literal) {
        name.to_string()
    } else {
        tracing::debug!("Skipping find_package with non-literal package name");
        return;
    };

    let mut version = None;
    let mut required = false;
    let mut components = Vec::new();
    let mut in_components = false;

    for arg in cmd.arguments.iter().skip(1) {
        let Some(lit) = arg.as_literal() else {
            continue;
        };
        let upper = lit.to_uppercase();
        match upper.as_str() {
            "REQUIRED" => required = true,
            "COMPONENTS" | "OPTIONAL_COMPONENTS" => in_components = true,
            "CONFIG" | "MODULE" | "NO_MODULE" | "QUIET" | "EXACT" => in_components = false,
            _ => {
                if in_components {
                    components.push(lit.to_string());
                } else if version.is_none() {
                    // First non-keyword arg after name might be version.
                    version = Some(lit.to_string());
                }
            }
        }
    }

    project.packages.push(Package {
        name,
        version,
        required,
        components,
    });
}

/// Extract pkg_check_modules() command.
pub fn extract_pkg_config(cmd: &Command, project: &mut CMakeProject, ctx: &mut EvalContext) {
    // pkg_check_modules(PREFIX [REQUIRED] [QUIET] pkg1 pkg2 ...)
    let args: Vec<&str> = cmd
        .arguments
        .iter()
        .filter_map(|a| a.as_literal())
        .collect();

    if args.is_empty() {
        return;
    }

    let prefix = args[0].to_string();
    let mut required = false;
    let mut packages = Vec::new();

    for arg in args.iter().skip(1) {
        match arg.to_uppercase().as_str() {
            "REQUIRED" => required = true,
            "QUIET" | "NO_CMAKE_PATH" | "NO_CMAKE_ENVIRONMENT_PATH" | "IMPORTED_TARGET" => {}
            _ => packages.push((*arg).to_string()),
        }
    }

    if !packages.is_empty() {
        // Set placeholder values so variable expansion works
        // e.g., ${GLIB_LIBRARIES} -> ["-lglib-2.0"]
        let lib_flags: Vec<String> = packages
            .iter()
            .map(|p| {
                // Strip version specifier (e.g., "glib-2.0>=2.44.0" -> "glib-2.0")
                let pkg_name = strip_version_specifier(p);
                format!("-l{pkg_name}")
            })
            .collect();

        ctx.set(&format!("{prefix}_LIBRARIES"), lib_flags);
        ctx.set(&format!("{prefix}_INCLUDE_DIRS"), vec![]);

        project.pkg_config_modules.push(PkgConfigModule {
            prefix,
            packages,
            required,
        });
    }
}

/// Strip version specifiers from a pkg-config package name.
/// E.g., "glib-2.0>=2.44.0" -> "glib-2.0"
fn strip_version_specifier(pkg: &str) -> &str {
    let pkg_name = pkg.split(">=").next().unwrap_or(pkg);
    let pkg_name = pkg_name.split("<=").next().unwrap_or(pkg_name);
    let pkg_name = pkg_name.split('>').next().unwrap_or(pkg_name);
    let pkg_name = pkg_name.split('<').next().unwrap_or(pkg_name);
    pkg_name.split('=').next().unwrap_or(pkg_name)
}
