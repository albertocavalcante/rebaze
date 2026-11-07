def _system_lib_repo_impl(repository_ctx):
    """Implementation of system library repository rule."""
    name = repository_ctx.attr.lib_name
    path = repository_ctx.attr.path
    include_dirs = repository_ctx.attr.include_dirs
    lib_dirs = repository_ctx.attr.lib_dirs
    libs = repository_ctx.attr.libs

    # Symlink the system library directory
    repository_ctx.symlink(path, "root")

    # Generate BUILD file content
    hdrs_patterns = []
    for d in include_dirs:
        if d.startswith("/"):
            # Absolute path - can't glob, skip
            continue
        hdrs_patterns.append("root/{}/**/*.h".format(d))

    hdrs_glob = "glob({}, allow_empty = True)".format(json.encode(hdrs_patterns)) if hdrs_patterns else "[]"
    includes_list = ["root/{}".format(d) for d in include_dirs if not d.startswith("/")]

    # Build linkopts: -L for library directories, -l for libraries
    # NOTE: We use absolute paths since we're already non-hermetic with system libs
    linkopts = []
    for d in lib_dirs:
        if d.startswith("/"):
            # Already absolute
            linkopts.append("-L{}".format(d))
        else:
            # Relative to prefix - construct absolute path
            linkopts.append("-L{}/{}".format(path, d))
    for l in libs:
        linkopts.append("-l{}".format(l))

    build_content = '''
load("@rules_cc//cc:defs.bzl", "cc_library")

cc_library(
    name = {name},
    hdrs = {hdrs},
    includes = {includes},
    linkopts = {linkopts},
    visibility = ["//visibility:public"],
)
'''.format(
        name = json.encode(name),
        hdrs = hdrs_glob,
        includes = json.encode(includes_list),
        linkopts = json.encode(linkopts),
    )

    repository_ctx.file("BUILD.bazel", build_content)

_system_lib_repo = repository_rule(
    implementation = _system_lib_repo_impl,
    attrs = {
        "lib_name": attr.string(mandatory = True),
        "path": attr.string(mandatory = True),
        "include_dirs": attr.string_list(default = ["include"]),
        "lib_dirs": attr.string_list(default = ["lib"]),
        "libs": attr.string_list(default = []),
    },
    local = True,  # Depends on local system state
)

def _stub_repo_impl(repository_ctx):
    """Create a stub repository for unavailable packages."""
    name = repository_ctx.attr.lib_name
    build_content = '''
load("@rules_cc//cc:defs.bzl", "cc_library")

# Stub library - package not available via pkg-config during migration
# Configure the real path in system_deps.bzl SYSTEM_LIBS
cc_library(
    name = {name},
    visibility = ["//visibility:public"],
)
'''.format(name = json.encode(name))
    repository_ctx.file("BUILD.bazel", build_content)

_stub_repo = repository_rule(
    implementation = _stub_repo_impl,
    attrs = {
        "lib_name": attr.string(mandatory = True),
    },
)

def _system_deps_impl(module_ctx):
    """Implementation of system_deps module extension."""
    for name, config in SYSTEM_LIBS.items():
        if "path" not in config:
            # Create stub repo for unavailable packages
            _stub_repo(
                name = "system_" + name,
                lib_name = name,
            )
            continue

        _system_lib_repo(
            name = "system_" + name,
            lib_name = name,
            path = config["path"],
            include_dirs = config.get("include_dirs", ["include"]),
            lib_dirs = config.get("lib_dirs", ["lib"]),
            libs = config.get("libs", [name]),
        )

system_deps = module_extension(
    implementation = _system_deps_impl,
)
