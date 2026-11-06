def resolve_dep(name):
    """Resolve a dependency based on configuration.

    Args:
        name: The dependency name (e.g., "glib")

    Returns:
        A label pointing to the resolved dependency target.
    """
    config = DEPS_CONFIG.get(name, {})
    strategy = config.get("strategy", DEFAULT_STRATEGY)

    if strategy == "bcr":
        # Use BCR module via bazel_dep (requires entry in MODULE.bazel)
        bcr_module = config.get("bcr_module", name)
        bcr_target = config.get("bcr_target", "")
        if bcr_target:
            return bcr_target
        return "@{}".format(bcr_module)
    elif strategy == "system":
        # Use new_local_repository wrapper from system_deps module extension
        return "@system_{}//:{}".format(name, name)
    elif strategy == "source":
        return "//third_party:{}_source".format(name)
    elif strategy == "conan":
        return "@conan_{}//:{}".format(name, name)
    elif strategy == "vcpkg":
        return "@vcpkg_{}//:{}".format(name, name)
    elif strategy == "prebuilt":
        return "//third_party:{}_prebuilt".format(name)
    elif strategy == "bazelle":
        # Use bazelle/gazelle_cc generated BUILD files
        return "@{}_src//:{}".format(name, name)
    elif strategy == "custom":
        # Users implement custom_resolve_dep in config_custom.bzl
        return custom_resolve_dep(name, config)
    else:
        fail("Unknown dependency strategy: " + strategy)

def custom_resolve_dep(name, config):
    """Override this function in config_custom.bzl for custom resolution.

    Example config_custom.bzl:
        def custom_resolve_dep(name, config):
            return "@my_internal_repo//libs:" + name
    """
    fail("Custom resolution not implemented. Create config_custom.bzl and implement custom_resolve_dep()")
