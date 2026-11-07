def _bazel_option(value):
    # linkopts undergo Make expansion and Bourne tokenization before reaching the linker.
    value = value.replace("$", "$$")
    return "'" + value.replace("'", "'\\''") + "'"

def _system_lib_repo_impl(repository_ctx):
    """Expose observed include roots and preserve linker argument order."""
    includes = []
    patterns = []
    for index, directory in enumerate(repository_ctx.attr.include_dirs):
        path = directory if directory.startswith("/") else repository_ctx.attr.path + "/" + directory
        local = "include_{}".format(index)
        repository_ctx.symlink(path, local)
        includes.append(local)
        patterns.append(local + "/**")

    # Defines propagate to consumers; arbitrary compile options remain reviewable snapshot data.
    defines = [flag[2:] for flag in repository_ctx.attr.cflags if flag.startswith("-D")]
    build_content = '''
load("@rules_cc//cc:defs.bzl", "cc_library")

cc_library(
    name = {name},
    hdrs = glob({patterns}, allow_empty = True),
    includes = {includes},
    defines = {defines},
    linkopts = {linkopts},
    visibility = ["//visibility:public"],
)
'''.format(
        name = json.encode(repository_ctx.attr.lib_name),
        patterns = json.encode(patterns),
        includes = json.encode(includes),
        defines = json.encode([_bazel_option(value) for value in defines]),
        linkopts = json.encode([_bazel_option(value) for value in repository_ctx.attr.linkopts]),
    )
    repository_ctx.file("BUILD.bazel", build_content)

_system_lib_repo = repository_rule(
    implementation = _system_lib_repo_impl,
    attrs = {
        "lib_name": attr.string(mandatory = True),
        "path": attr.string(mandatory = True),
        "include_dirs": attr.string_list(),
        "cflags": attr.string_list(),
        "linkopts": attr.string_list(),
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
            cflags = config.get("cflags", []),
            linkopts = config.get("linkopts", []),
        )

system_deps = module_extension(
    implementation = _system_deps_impl,
)
