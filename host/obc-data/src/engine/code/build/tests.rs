use super::*;
use crate::engine::code::rust::Packages;
use crate::engine::tests::{fixture, repository, write};

#[test]
fn native_library_bindings_keep_original_bytes_and_recheck_the_current_files() {
    use crate::engine::Library;
    let fixture = fixture("code-libraries");
    let root = fixture.root();
    let files = [root.join("c-library"), root.join("cpp-library")];
    for file in &files {
        write(file, "initial implementation");
    }
    let mut code = Code {
        libraries: ["geos-c", "geos-cpp"]
            .into_iter()
            .zip(&files)
            .map(|(name, file)| Library {
                name: name.into(),
                path: file.canonicalize().unwrap(),
                sha256: hash_file(file).unwrap().0,
            })
            .collect(),
        ..Default::default()
    };
    let mut context = crate::engine::code::Context::default();
    let before = context.files(&root, &code).unwrap();
    assert_eq!(context.files(&root, &code).unwrap(), before);
    assert_eq!(context.build.tools.len(), 2, "unchanged library bytes reuse the operation cache");
    let copy = root.join("same-library-elsewhere");
    std::fs::copy(&files[0], &copy).unwrap();
    code.libraries[0].path = copy.canonicalize().unwrap();
    assert_eq!(context.files(&root, &code).unwrap(), before, "installation paths do not enter byte identity");
    write(&copy, "changed implementation");
    assert!(context.files(&root, &code).unwrap_err().contains("changed; start a fresh worker"));
    write(&copy, "initial implementation");
    std::fs::remove_file(&files[1]).unwrap();
    assert!(context.files(&root, &code).unwrap_err().contains("start a fresh worker"));
}

#[test]
fn selected_profile_keeps_applicable_package_and_build_overrides() {
    let packages = Packages { names: ["producer".into(), "dependency".into()].into(), non_workspace: true };
    let text = "[profile.dev]\ndebug = 1\n[profile.dev.package.producer]\nopt-level = 2\n[profile.dev.package.other]\nopt-level = 3\n[profile.dev.package.'*']\ndebug = 0\n[profile.dev.build-override]\nopt-level = 1\n[profile.release]\nopt-level = 3\n";
    let project = |text: &str| profile_projection(text, Profile::Dev, &packages).unwrap();
    let before = project(text);
    assert_eq!(project(&text.replace("opt-level = 3", "opt-level = 2")), before, "unselected package/release profile");
    for changed in [
        text.replacen("debug = 1", "debug = 2", 1),
        text.replacen("opt-level = 2", "opt-level = 1", 1),
        text.replacen("debug = 0", "debug = 2", 1),
        text.replacen("opt-level = 1", "opt-level = 0", 1),
    ] {
        assert_ne!(project(&changed), before);
    }
    assert_ne!(profile_projection(text, Profile::Release, &packages).unwrap(), before);
    let workspace_only = Packages { non_workspace: false, ..packages };
    assert_eq!(
        profile_projection(text, Profile::Dev, &workspace_only).unwrap(),
        profile_projection(&text.replace("debug = 0", "debug = 2"), Profile::Dev, &workspace_only).unwrap(),
        "wildcard overrides apply only to non-workspace packages"
    );
    for unsupported in ["[profile.dev]\ninherits = 'release'", "[profile.dev.package.'producer:1.0.0']\nopt-level = 2"]
    {
        assert!(profile_projection(unsupported, Profile::Dev, &workspace_only).is_err());
    }
    let hashing = Packages { names: ["sha2".into()].into(), non_workspace: true };
    let optimized = "[profile.dev.package.sha2]\nopt-level=3\n";
    assert_ne!(
        profile_projection(optimized, Profile::Dev, &hashing).unwrap(),
        profile_projection("", Profile::Dev, &hashing).unwrap()
    );
    assert_eq!(
        profile_projection(optimized, Profile::Release, &hashing).unwrap(),
        profile_projection("", Profile::Release, &hashing).unwrap()
    );
}

#[test]
fn ordered_flags_follow_cargo_precedence_and_refuse_external_or_host_dependent_inputs() {
    let mut env = BTreeMap::new();
    env.insert("RUSTFLAGS".into(), "-Copt-level=2 --cfg selected -C target-cpu=cortex-a53".into());
    let plain = flags(&env).unwrap();
    assert_eq!(plain, ["-Copt-level=2", "--cfg", "selected", "-C", "target-cpu=cortex-a53"]);
    env.insert("CARGO_ENCODED_RUSTFLAGS".into(), "--cfg\u{1f}selected=\"a b\"\u{1f}-Copt-level=3".into());
    let encoded = flags(&env).unwrap();
    env.insert("RUSTFLAGS".into(), "-Ctarget-cpu=native".into());
    assert_eq!(flags(&env).unwrap(), encoded, "lower-priority flags have no effect");
    env.insert("CARGO_ENCODED_RUSTFLAGS".into(), "".into());
    assert!(flags(&env).unwrap().is_empty(), "explicit empty encoded flags suppress RUSTFLAGS");
    env.remove(OsStr::new("CARGO_ENCODED_RUSTFLAGS"));
    assert!(flags(&env).unwrap_err().contains("explicit target CPU"));
    for flag in
        ["-Clinker=/tmp/tool", "--sysroot /tmp/root", "-Cprofile-use=/tmp/profile", "-L /tmp/libs", "-Zunstable"]
    {
        env.insert("RUSTFLAGS".into(), flag.into());
        assert!(flags(&env).is_err(), "{flag}");
    }
    env.insert("RUSTFLAGS".into(), "-Copt-level=2 -Copt-level=3".into());
    let forward = digest(&flags(&env).unwrap());
    env.insert("RUSTFLAGS".into(), "-Copt-level=3 -Copt-level=2".into());
    assert_ne!(digest(&flags(&env).unwrap()), forward, "flag order is part of the effective invocation");
}

#[test]
fn cargo_config_discovery_and_refusals_do_not_expose_values() {
    let fixture = fixture("build-config");
    let root = fixture.root();
    let parent = root.parent().unwrap();
    let cargo_home = root.join("tool-home");
    let env = BTreeMap::from([("CARGO_HOME".into(), cargo_home.as_os_str().into())]);
    write(&parent.join(".cargo/config"), "[build]\njobs = 2\n");
    write(&parent.join(".cargo/config.toml"), "[build]\nrustflags = ['secret-value']\n");
    let paths = configs(&root, &env);
    assert!(paths.contains(&parent.join(".cargo/config")));
    assert!(!paths.contains(&parent.join(".cargo/config.toml")), "Cargo gives the extensionless file priority");
    assert!(paths.contains(&cargo_home.join("config.toml")));
    validate_config("[build]\njobs = 2\ntarget-dir = '/tmp/cache'\n[net]\noffline = true\n[registries.private]\ntoken = 'secret-value'\n").unwrap();
    for text in [
        "[env]\nSECRET = 'secret-value'",
        "[profile.dev]\nopt-level = 3",
        "[target.'cfg(unix)']\nrustflags = ['secret-value']",
        "[build]\nrustc-wrapper = 'secret-value'",
    ] {
        let error = validate_config(text).unwrap_err();
        assert!(!error.contains("secret-value"));
    }
    for name in [
        "RUSTC_WRAPPER",
        "CARGO_PROFILE_CUSTOM_OPT_LEVEL",
        "CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER",
        "CFLAGS",
        "CC_aarch64_unknown_linux_gnu",
        "HOST_CC",
        "HOST_CFLAGS",
        "TARGET_CC",
        "TARGET_CXXFLAGS",
        "TARGET_ARFLAGS",
        "ARFLAGS",
        "HOST_RANLIBFLAGS",
        "TARGET_RANLIBFLAGS",
        "LIBRARY_PATH",
        "LD_PRELOAD",
        "LD_AUDIT",
    ] {
        let env = BTreeMap::from([(name.into(), "secret-value".into())]);
        let error = validate_environment(&env).unwrap_err();
        assert!(error.contains(name));
        assert!(!error.contains("secret-value"));
    }
    validate_environment(&BTreeMap::from([
        ("CARGO_BUILD_JOBS".into(), "2".into()),
        ("CARGO_TARGET_DIR".into(), "/tmp/cache".into()),
        ("RUSTC_WRAPPER".into(), "".into()),
        ("LD_LIBRARY_PATH".into(), "/target/debug/deps:/rust/sysroot/lib:/runtime/geos".into()),
    ]))
    .unwrap();
}

#[test]
fn existing_ci_profile_and_lint_settings_are_bound_to_the_selected_build() {
    let env = BTreeMap::from([
        ("CARGO_PROFILE_DEV_DEBUG".into(), "0".into()),
        ("CARGO_INCREMENTAL".into(), "0".into()),
        ("RUSTFLAGS".into(), "-D warnings".into()),
    ]);
    validate_environment(&env).unwrap();
    assert_eq!(flags(&env).unwrap(), ["-D", "warnings"]);
    let settings = BTreeMap::from([
        ("CARGO_PROFILE_DEV_DEBUG".into(), "0".into()),
        ("CARGO_PROFILE_RELEASE_DEBUG".into(), "2".into()),
        ("CARGO_PROFILE_DEV_BUILD_OVERRIDE_DEBUG".into(), "1".into()),
        ("CARGO_INCREMENTAL".into(), "0".into()),
    ]);
    let before = digest(&profile_environment(Profile::Dev, &settings));
    let mut changed = settings.clone();
    changed.insert("CARGO_PROFILE_RELEASE_DEBUG".into(), "1".into());
    assert_eq!(digest(&profile_environment(Profile::Dev, &changed)), before);
    for name in ["CARGO_PROFILE_DEV_DEBUG", "CARGO_PROFILE_DEV_BUILD_OVERRIDE_DEBUG", "CARGO_INCREMENTAL"] {
        let mut changed = settings.clone();
        changed.insert(name.into(), "3".into());
        assert_ne!(digest(&profile_environment(Profile::Dev, &changed)), before, "{name}");
    }
}

#[test]
fn native_dev_alias_profile_changes_and_plan_use_the_same_identity() {
    let fixture = fixture("native-build-code");
    let root = fixture.root();
    repository(&root, &[("steps", ""), ("other", "")]);
    let code = Code { crates: vec!["steps".into()], ..Default::default() };
    let mut context = Context::default();
    let packages = Packages { names: ["steps".into()].into(), non_workspace: false };
    let implicit = context.identity(&root, &code, &packages).unwrap();
    let explicit = Code { rust: Some(Rust::Native { profile: Profile::Dev }), ..code.clone() };
    assert_eq!(context.identity(&root, &explicit, &packages).unwrap(), implicit);
    assert!(implicit.files.contains_key("rust/compiler") && implicit.files.contains_key("rust/target"));
    assert!(implicit.files.keys().any(|name| name.starts_with("rust/sysroot-library/libstd")));
    assert!(context.native.is_some());
    let tools = context.tools.len();
    context.identity(&root, &code, &packages).unwrap();
    assert_eq!(context.tools.len(), tools, "one context reuses compiler fingerprints");

    let steps = crate::engine::tests::pipeline();
    fixture.build(&steps).unwrap();
    assert!(fixture.plan(&steps).unwrap().groups.is_empty());
    let manifest = root.join("Cargo.toml");
    let original = fs::read_to_string(&manifest).unwrap();
    write(
        &manifest,
        &(original.clone() + "\n[profile.release]\ndebug = 2\n[profile.dev.package.other]\nopt-level = 2\n"),
    );
    assert_eq!(context.identity(&root, &code, &packages).unwrap(), implicit);
    assert!(fixture.plan(&steps).unwrap().groups.is_empty());
    write(&manifest, &(original + "\n[profile.dev.package.steps]\nopt-level = 2\n"));
    assert_ne!(context.identity(&root, &code, &packages).unwrap(), implicit);
    assert!(!fixture.plan(&steps).unwrap().groups.is_empty());
    let release = Code { rust: Some(Rust::Native { profile: Profile::Release }), ..code };
    assert_ne!(context.identity(&root, &release, &packages).unwrap(), implicit);

    let mut checks = crate::engine::code::Context::default();
    let files = checks.files(&root, &explicit).unwrap();
    let probes = checks.build.probes;
    checks.refresh_python();
    assert_eq!(checks.files(&root, &explicit).unwrap(), files);
    assert_eq!(checks.build.probes, probes, "wired pre/post checks reuse tool discovery");
    write(&root.join(".cargo/config.toml"), "[build]\njobs = 2\n");
    checks.refresh_python();
    assert_eq!(checks.files(&root, &explicit).unwrap(), files);
    assert_eq!(checks.build.probes, probes + 1, "a changed selection/config witness revalidates discovery");
}

#[test]
fn tool_cache_detects_replaced_bytes_at_the_check_boundary() {
    let fixture = fixture("compiler-cache");
    let compiler = fixture.root().join("compiler");
    write(&compiler, "first executable");
    assert!(native_binary(&compiler).unwrap_err().contains("script wrappers"));
    let mut context = Context::default();
    let first = context.tool_hash(&compiler).unwrap();
    assert_eq!(context.tool_hash(&compiler).unwrap(), first);
    write(&compiler, "other executable");
    assert_ne!(context.tool_hash(&compiler).unwrap(), first);
    let lib = fixture.root().join("libstd-fixture.rlib");
    write(&lib, "standard library first");
    let first = context.library_hashes(&fixture.root()).unwrap();
    write(&lib, "standard library other");
    let second = context.library_hashes(&fixture.root()).unwrap();
    assert_ne!(first["rust/sysroot-library/libstd-fixture.rlib"], second["rust/sysroot-library/libstd-fixture.rlib"]);
    assert_eq!(first["rust/sysroot-library/compiler"], second["rust/sysroot-library/compiler"]);
}

#[cfg(unix)]
#[test]
fn runtime_loader_search_paths_do_not_select_native_compiler_libraries() {
    let runtime = || {
        let mut command = Command::new("sh");
        command
            .args(["-c", "printf '%s|%s' \"${LD_LIBRARY_PATH-}\" \"${DYLD_FALLBACK_LIBRARY_PATH-}\""])
            .env("LD_LIBRARY_PATH", "/target/debug/deps:/runtime/geos")
            .env("DYLD_FALLBACK_LIBRARY_PATH", "/target/debug/deps");
        command
    };
    let mut retained = runtime();
    for (name, value) in
        [("LD_LIBRARY_PATH", "/target/debug/deps:/runtime/geos"), ("DYLD_FALLBACK_LIBRARY_PATH", "/target/debug/deps")]
    {
        assert!(retained.get_envs().any(|(key, selected)| key == name && selected == Some(OsStr::new(value))));
    }
    // macOS protected executables remove DYLD variables before the script starts.
    let expected = if cfg!(target_os = "macos") {
        b"/target/debug/deps:/runtime/geos|".as_slice()
    } else {
        b"/target/debug/deps:/runtime/geos|/target/debug/deps".as_slice()
    };
    assert_eq!(retained.output().unwrap().stdout, expected);
    let mut compiler = runtime();
    crate::worker::compiler_command(&mut compiler);
    for name in ["LD_LIBRARY_PATH", "DYLD_FALLBACK_LIBRARY_PATH"] {
        assert!(compiler.get_envs().any(|(key, selected)| key == name && selected.is_none()));
    }
    assert_eq!(compiler.output().unwrap().stdout, b"|");

    let native = Code { crates: vec!["producer".into()], ..Default::default() };
    let prepared = Code { rust: Some(Rust::Prepared { profile: Profile::Release }), ..native.clone() };
    let ordinary = Code::default();
    for (program, code, normalized) in [
        ("cargo", &native, true),
        ("rustc", &native, true),
        ("obc-bake", &native, false),
        ("cargo", &prepared, false),
        ("cargo", &ordinary, false),
    ] {
        let command = crate::engine::process::command(Path::new("."), &[program.into()], Some((code, ""))).unwrap();
        assert_eq!(
            command.get_envs().any(|(name, value)| name == "LD_LIBRARY_PATH" && value.is_none()),
            normalized,
            "{program}"
        );
    }
}

#[cfg(unix)]
#[test]
fn rustup_proxy_resolves_the_selected_executable_in_the_explicit_root() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = fixture("selected-rustc");
    let root = fixture.root();
    let rustup = root.join("rustup");
    let rustc = root.join("rustc");
    let selected = root.join("actual-rustc");
    write(&selected, "actual compiler bytes");
    write(
        &rustup,
        "#!/bin/sh\n[ \"$1\" = which ] && [ \"$2\" = rustc ] || exit 1\nprintf '%s/actual-rustc\\n' \"$PWD\"\n",
    );
    fs::set_permissions(&rustup, fs::Permissions::from_mode(0o700)).unwrap();
    fs::hard_link(&rustup, &rustc).unwrap();
    assert_eq!(executable(&root, Some(&rustc.into_os_string()), "rustc").unwrap(), selected.canonicalize().unwrap());
}
