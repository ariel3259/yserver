use std::{
    collections::HashMap,
    fs, io,
    path::{Path, PathBuf},
};

use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VrrExclusionSource {
    Environment,
    NvidiaProfiles,
    MesaDriconf,
}

#[derive(Clone, Debug)]
pub struct VrrListRoots {
    pub nvidia_driver_dir: PathBuf,
    pub nvidia_etc_dir: PathBuf,
    pub mesa_system_dir: PathBuf,
    pub mesa_etc_file: PathBuf,
    pub home: PathBuf,
}

impl VrrListRoots {
    #[must_use]
    pub fn system() -> Self {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/nonexistent"));
        Self {
            nvidia_driver_dir: PathBuf::from("/usr/share/nvidia"),
            nvidia_etc_dir: PathBuf::from("/etc/nvidia"),
            mesa_system_dir: PathBuf::from("/usr/share/drirc.d"),
            mesa_etc_file: PathBuf::from("/etc/drirc"),
            home,
        }
    }
}

pub struct VrrExclusions {
    nvidia: HashMap<String, bool>,
    mesa: HashMap<String, bool>,
}

pub(crate) struct VrrExclusionDecision {
    pub(crate) process_name: Option<String>,
    pub(crate) source: Option<VrrExclusionSource>,
}

impl VrrExclusions {
    #[must_use]
    pub fn load(roots: &VrrListRoots) -> Self {
        let mut nvidia_documents = Vec::new();
        let user_nvidia = roots.home.join(".nv");
        let mut nvidia_paths = vec![user_nvidia.join("nvidia-application-profiles-rc")];
        nvidia_paths.extend(files_in_dir(
            &user_nvidia.join("nvidia-application-profiles-rc.d"),
        ));
        nvidia_paths.push(roots.nvidia_etc_dir.join("nvidia-application-profiles-rc"));
        nvidia_paths.extend(files_in_dir(
            &roots
                .nvidia_etc_dir
                .join("nvidia-application-profiles-rc.d"),
        ));
        nvidia_paths.extend(files_named(&roots.nvidia_driver_dir, |name| {
            name.starts_with("nvidia-application-profiles-") && name.ends_with("-rc")
        }));
        for path in nvidia_paths {
            if let Some(contents) = read_optional(&path, "NVIDIA application profile") {
                if let Some(document) = parse_nvidia_document(&contents) {
                    nvidia_documents.push(document);
                } else {
                    log::warn!(
                        "kms: skipping malformed NVIDIA application profile {}",
                        path.display()
                    );
                }
            }
        }
        let nvidia = parse_nvidia_profiles(&nvidia_documents);

        let mut mesa = HashMap::new();
        let mesa_system_paths = files_named(&roots.mesa_system_dir, |name| name.ends_with(".conf"));
        if mesa_system_paths.is_empty() {
            log::warn!(
                "kms: vrr all requires Mesa's {} (Mesa DRI package: Arch/Alpine mesa, Debian/Ubuntu libgl1-mesa-dri, Fedora mesa-dri-drivers, FreeBSD mesa-dri, Gentoo media-libs/mesa)",
                roots
                    .mesa_system_dir
                    .join("00-mesa-defaults.conf")
                    .display()
            );
        }
        let mut mesa_paths = mesa_system_paths;
        mesa_paths.push(roots.mesa_etc_file.clone());
        mesa_paths.push(roots.home.join(".drirc"));
        for path in mesa_paths {
            if let Some(contents) = read_optional(&path, "Mesa driconf") {
                if let Some(entries) = parse_driconf(&contents) {
                    mesa.extend(entries);
                } else {
                    log::warn!("kms: skipping malformed Mesa driconf {}", path.display());
                }
            }
        }

        let nvidia_count = nvidia.values().filter(|allowed| !**allowed).count();
        let mesa_count = mesa.values().filter(|allowed| !**allowed).count();
        log::info!("kms: vrr exclusions nvidia={nvidia_count} mesa={mesa_count}");
        Self { nvidia, mesa }
    }

    /// Return the list source that excludes this process, or `None` when it
    /// may use VRR. The server passes `/proc` in production and a synthetic
    /// proc tree in tests.
    #[must_use]
    pub fn excluded(&self, pid: u32, proc_root: &Path) -> Option<VrrExclusionSource> {
        self.evaluate(pid, proc_root).source
    }

    pub(crate) fn evaluate(&self, pid: u32, proc_root: &Path) -> VrrExclusionDecision {
        let process_dir = proc_root.join(pid.to_string());
        let environment = read_environment(&process_dir.join("environ"));
        let nvidia_environment = environment
            .get("__GL_VRR_ALLOWED")
            .and_then(|value| parse_bool(value));
        let mesa_environment = environment
            .get("adaptive_sync")
            .and_then(|value| parse_bool(value));
        let process_name = read_process_name(&process_dir);
        let source = if nvidia_environment == Some(false) || mesa_environment == Some(false) {
            Some(VrrExclusionSource::Environment)
        } else if nvidia_environment == Some(true) || mesa_environment == Some(true) {
            None
        } else {
            process_name.as_ref().and_then(|name| {
                if self.nvidia.get(name) == Some(&false) {
                    Some(VrrExclusionSource::NvidiaProfiles)
                } else if self.mesa.get(name) == Some(&false) {
                    Some(VrrExclusionSource::MesaDriconf)
                } else {
                    None
                }
            })
        };
        VrrExclusionDecision {
            process_name,
            source,
        }
    }
}

fn files_in_dir(directory: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut paths: Vec<_> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .collect();
    paths.sort();
    paths
}

fn files_named(directory: &Path, matches: impl Fn(&str) -> bool) -> Vec<PathBuf> {
    files_in_dir(directory)
        .into_iter()
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(&matches)
        })
        .collect()
}

fn read_optional(path: &Path, description: &str) -> Option<String> {
    match fs::read_to_string(path) {
        Ok(contents) => Some(contents),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => {
            log::warn!("kms: cannot read {description} {}: {error}", path.display());
            None
        }
    }
}

fn parse_nvidia_document(contents: &str) -> Option<Value> {
    let json = drop_hash_comment_lines(contents);
    let document: Value = serde_json::from_str(&json).ok()?;
    document.as_object()?;
    Some(document)
}

fn parse_nvidia_profiles(documents: &[Value]) -> HashMap<String, bool> {
    let mut profiles: HashMap<&str, &Value> = HashMap::new();
    for document in documents {
        let Some(items) = document.get("profiles").and_then(Value::as_array) else {
            continue;
        };
        for item in items {
            if let (Some(name), Some(profile)) = (
                item.get("name").and_then(Value::as_str),
                item.get("settings"),
            ) {
                profiles.entry(name).or_insert(profile);
            }
        }
    }

    let mut settings_by_process = HashMap::new();
    for document in documents {
        let Some(rules) = document.get("rules").and_then(Value::as_array) else {
            continue;
        };
        for rule in rules {
            let Some(process_name) = rule.get("pattern").and_then(procname_pattern) else {
                continue;
            };
            let Some(profile) = rule.get("profile") else {
                continue;
            };
            let settings = match profile {
                Value::String(name) => profiles.get(name.as_str()).copied(),
                Value::Object(object) => object.get("settings"),
                settings @ Value::Array(_) => Some(settings),
                _ => None,
            };
            let Some(allowed) =
                settings.and_then(|settings| setting_bool(settings, "GLVRRAllowed"))
            else {
                continue;
            };
            // NVIDIA applies earlier matching rules before later ones. The
            // first value for this key therefore wins across files and rules.
            settings_by_process
                .entry(process_name.to_owned())
                .or_insert(allowed);
        }
    }
    settings_by_process
}

fn procname_pattern(pattern: &Value) -> Option<&str> {
    match pattern {
        Value::String(name) => Some(name),
        Value::Object(pattern)
            if pattern.get("feature").and_then(Value::as_str) == Some("procname") =>
        {
            pattern.get("matches").and_then(Value::as_str)
        }
        _ => None,
    }
}

fn setting_bool(settings: &Value, key: &str) -> Option<bool> {
    match settings {
        Value::Array(entries) => {
            if entries.iter().any(Value::is_object) {
                entries.iter().find_map(|entry| {
                    let entry_key = entry
                        .get("key")
                        .or_else(|| entry.get("k"))
                        .and_then(Value::as_str);
                    (entry_key == Some(key)).then(|| {
                        entry
                            .get("value")
                            .or_else(|| entry.get("v"))
                            .and_then(value_bool)
                    })?
                })
            } else {
                entries.chunks_exact(2).find_map(|pair| {
                    (pair[0].as_str() == Some(key))
                        .then(|| value_bool(&pair[1]))
                        .flatten()
                })
            }
        }
        _ => None,
    }
}

fn value_bool(value: &Value) -> Option<bool> {
    match value {
        Value::Bool(value) => Some(*value),
        Value::Number(value) => match value.as_i64()? {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        },
        Value::String(value) => parse_bool(value),
        _ => None,
    }
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "0" | "false" => Some(false),
        "1" | "true" => Some(true),
        _ => None,
    }
}

fn drop_hash_comment_lines(source: &str) -> String {
    source
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n")
}

fn read_environment(path: &Path) -> HashMap<String, String> {
    let Ok(contents) = fs::read(path) else {
        return HashMap::new();
    };
    contents
        .split(|byte| *byte == 0)
        .filter_map(|entry| {
            let entry = std::str::from_utf8(entry).ok()?;
            let (key, value) = entry.split_once('=')?;
            matches!(key, "__GL_VRR_ALLOWED" | "adaptive_sync")
                .then(|| (key.to_owned(), value.to_owned()))
        })
        .collect()
}

fn read_process_name(process_dir: &Path) -> Option<String> {
    use std::os::unix::ffi::OsStrExt;

    let cmdline = fs::read(process_dir.join("cmdline")).ok()?;
    let argv0 = cmdline.split(|byte| *byte == 0).next()?;
    let argv0 = std::str::from_utf8(argv0).ok()?;
    let argv_name = Path::new(argv0)
        .file_name()
        .and_then(|name| name.to_str())
        .map(ToOwned::to_owned);
    let executable_text = fs::read_link(process_dir.join("exe"))
        .ok()
        .and_then(|executable| {
            std::str::from_utf8(executable.as_os_str().as_bytes())
                .ok()
                .map(ToOwned::to_owned)
        });
    executable_text
        .filter(|executable| argv0.starts_with(executable))
        .and_then(|executable| {
            Path::new(&executable)
                .file_name()
                .and_then(|name| name.to_str())
                .map(ToOwned::to_owned)
        })
        .or(argv_name)
}

fn parse_driconf(contents: &str) -> Option<HashMap<String, bool>> {
    let document = roxmltree::Document::parse(contents).ok()?;
    let mut settings = HashMap::new();
    for application in document
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "application")
    {
        let Some(name) = application.attribute("executable") else {
            continue;
        };
        for option in application
            .descendants()
            .filter(|node| node.is_element() && node.tag_name().name() == "option")
        {
            if option.attribute("name") == Some("adaptive_sync")
                && let Some(value) = option.attribute("value").and_then(parse_bool)
            {
                settings.insert(name.to_owned(), value);
            }
        }
    }
    Some(settings)
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::VrrListRoots;
    use std::{
        fs,
        path::{Path, PathBuf},
        sync::atomic::{AtomicU64, Ordering},
    };

    static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(0);

    pub(crate) struct TempTree(PathBuf);

    impl TempTree {
        pub(crate) fn new() -> Self {
            let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
            let path =
                std::env::temp_dir().join(format!("yserver-vrr-test-{}-{id}", std::process::id()));
            fs::create_dir_all(&path).expect("create temp fixture root");
            Self(path)
        }

        pub(crate) fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    pub(crate) fn roots(tree: &TempTree) -> VrrListRoots {
        VrrListRoots {
            nvidia_driver_dir: tree.path().join("usr/share/nvidia"),
            nvidia_etc_dir: tree.path().join("etc/nvidia"),
            mesa_system_dir: tree.path().join("usr/share/drirc.d"),
            mesa_etc_file: tree.path().join("etc/drirc"),
            home: tree.path().join("home/user"),
        }
    }

    pub(crate) fn write(path: impl AsRef<Path>, contents: &str) {
        let path = path.as_ref();
        fs::create_dir_all(path.parent().expect("fixture path has parent"))
            .expect("create fixture parent");
        fs::write(path, contents).expect("write fixture file");
    }

    pub(crate) fn process(
        proc_root: &Path,
        pid: u32,
        executable: &str,
        argv0: &str,
        environ: &[&str],
    ) {
        use std::os::unix::fs::symlink;

        let process_dir = proc_root.join(pid.to_string());
        let _ = fs::remove_dir_all(&process_dir);
        fs::create_dir_all(&process_dir).expect("create fake process directory");
        symlink(executable, process_dir.join("exe")).expect("create fake executable symlink");
        let mut cmdline = argv0.as_bytes().to_vec();
        cmdline.push(0);
        fs::write(process_dir.join("cmdline"), cmdline).expect("write fake command line");
        fs::write(process_dir.join("environ"), environ.join("\0") + "\0")
            .expect("write fake environment");
    }
}

#[cfg(test)]
mod tests {
    use super::{
        VrrExclusionSource, VrrExclusions,
        test_support::{TempTree, process, roots, write},
    };

    #[test]
    // Mutation: ignore Mesa adaptive_sync=false entries.
    fn vrr_exclusions_mesa_entry_excludes() {
        let tree = TempTree::new();
        let roots = roots(&tree);
        let proc_root = tree.path().join("proc");
        write(
            roots.mesa_system_dir.join("10-app.conf"),
            r#"<driconf><device><application name="Browser" executable="firefox"><option name="adaptive_sync" value="false"/></application></device></driconf>"#,
        );
        process(&proc_root, 21, "/usr/lib/firefox", "/usr/lib/firefox", &[]);

        assert_eq!(
            VrrExclusions::load(&roots).excluded(21, &proc_root),
            Some(VrrExclusionSource::MesaDriconf)
        );
    }

    #[test]
    // Mutation: accept only object patterns and ignore NVIDIA string shorthand.
    fn vrr_exclusions_nvidia_procname_excludes() {
        let tree = TempTree::new();
        let roots = roots(&tree);
        let proc_root = tree.path().join("proc");
        write(
            roots
                .nvidia_driver_dir
                .join("nvidia-application-profiles-580-rc"),
            r##"{
          "rules": [
            {"pattern":{"feature":"procname","matches":"firefox"},"profile":["GLVRRAllowed",false]},
            {"pattern":"steam","profile":["GLVRRAllowed",false]}
          ]
        }"##,
        );
        process(&proc_root, 22, "/usr/lib/firefox", "/usr/lib/firefox", &[]);
        process(&proc_root, 23, "/home/user/steam", "steam", &[]);
        let exclusions = VrrExclusions::load(&roots);

        assert_eq!(
            exclusions.excluded(22, &proc_root),
            Some(VrrExclusionSource::NvidiaProfiles)
        );
        assert_eq!(
            exclusions.excluded(23, &proc_root),
            Some(VrrExclusionSource::NvidiaProfiles)
        );
    }

    #[test]
    // Mutation: default an unlisted process to excluded.
    fn vrr_exclusions_unlisted_allowed() {
        let tree = TempTree::new();
        let roots = roots(&tree);
        let proc_root = tree.path().join("proc");
        process(
            &proc_root,
            24,
            "/usr/bin/terminal",
            "/usr/bin/terminal",
            &[],
        );

        assert_eq!(VrrExclusions::load(&roots).excluded(24, &proc_root), None);
    }

    #[test]
    // Mutation: ignore later user files when resolving a name's setting.
    fn vrr_exclusions_later_file_lifts() {
        let tree = TempTree::new();
        let roots = roots(&tree);
        let proc_root = tree.path().join("proc");
        write(
            roots
                .nvidia_driver_dir
                .join("nvidia-application-profiles-580-rc"),
            r#"{"rules":[{"pattern":"firefox","profile":["GLVRRAllowed",false]}]}"#,
        );
        write(
            roots.home.join(".nv/nvidia-application-profiles-rc"),
            r#"{"rules":[{"pattern":"firefox","profile":["GLVRRAllowed",true]}]}"#,
        );
        write(
            roots.mesa_system_dir.join("10-firefox.conf"),
            r#"<driconf><device><application executable="firefox"><option name="adaptive_sync" value="false"/></application></device></driconf>"#,
        );
        write(
            roots.home.join(".drirc"),
            r#"<driconf><device><application executable="firefox"><option name="adaptive_sync" value="true"/></application></device></driconf>"#,
        );
        process(&proc_root, 25, "/usr/lib/firefox", "/usr/lib/firefox", &[]);

        assert_eq!(VrrExclusions::load(&roots).excluded(25, &proc_root), None);
    }

    #[test]
    // Mutation: skip profiles defined in later NVIDIA files.
    fn vrr_exclusions_nvidia_named_profile_cross_file() {
        let tree = TempTree::new();
        let roots = roots(&tree);
        let proc_root = tree.path().join("proc");
        write(
            roots.home.join(".nv/nvidia-application-profiles-rc"),
            r#"{"rules":[{"pattern":"firefox","profile":"deny"}]}"#,
        );
        write(
            roots
                .nvidia_driver_dir
                .join("nvidia-application-profiles-580-rc"),
            r#"{"profiles":[{"name":"deny","settings":["GLVRRAllowed",false]}]}"#,
        );
        process(&proc_root, 37, "/usr/lib/firefox", "/usr/lib/firefox", &[]);

        assert_eq!(
            VrrExclusions::load(&roots).excluded(37, &proc_root),
            Some(VrrExclusionSource::NvidiaProfiles)
        );
    }

    #[test]
    // Mutation: let a lower-precedence NVIDIA rule override a user rule referencing a later profile.
    fn vrr_exclusions_nvidia_named_profile_user_override() {
        let tree = TempTree::new();
        let roots = roots(&tree);
        let proc_root = tree.path().join("proc");
        write(
            roots.home.join(".nv/nvidia-application-profiles-rc"),
            r#"{"rules":[{"pattern":"firefox","profile":"allow"}]}"#,
        );
        write(
            roots
                .nvidia_driver_dir
                .join("nvidia-application-profiles-580-rc"),
            r#"{"profiles":[{"name":"allow","settings":["GLVRRAllowed",true]}]}"#,
        );
        write(
            roots
                .nvidia_driver_dir
                .join("nvidia-application-profiles-580.1-rc"),
            r#"{"rules":[{"pattern":"firefox","profile":["GLVRRAllowed",false]}]}"#,
        );
        process(&proc_root, 38, "/usr/lib/firefox", "/usr/lib/firefox", &[]);

        assert_eq!(VrrExclusions::load(&roots).excluded(38, &proc_root), None);
    }

    #[test]
    // Mutation: let NVIDIA file entries override environment values.
    fn vrr_exclusions_environment_wins() {
        let tree = TempTree::new();
        let roots = roots(&tree);
        let proc_root = tree.path().join("proc");
        write(
            roots
                .nvidia_driver_dir
                .join("nvidia-application-profiles-580-rc"),
            r#"{"rules":[{"pattern":"listed","profile":["GLVRRAllowed",false]}]}"#,
        );
        process(
            &proc_root,
            26,
            "/usr/bin/unlisted",
            "/usr/bin/unlisted",
            &["__GL_VRR_ALLOWED=0"],
        );
        process(
            &proc_root,
            27,
            "/usr/bin/listed",
            "/usr/bin/listed",
            &["__GL_VRR_ALLOWED=1"],
        );
        process(
            &proc_root,
            28,
            "/usr/bin/unlisted",
            "/usr/bin/unlisted",
            &["adaptive_sync=false"],
        );
        process(
            &proc_root,
            29,
            "/usr/bin/unlisted",
            "/usr/bin/unlisted",
            &["__GL_VRR_ALLOWED=1", "adaptive_sync=0"],
        );
        let exclusions = VrrExclusions::load(&roots);

        assert_eq!(
            exclusions.excluded(26, &proc_root),
            Some(VrrExclusionSource::Environment)
        );
        assert_eq!(exclusions.excluded(27, &proc_root), None);
        assert_eq!(
            exclusions.excluded(28, &proc_root),
            Some(VrrExclusionSource::Environment)
        );
        assert_eq!(
            exclusions.excluded(29, &proc_root),
            Some(VrrExclusionSource::Environment)
        );
    }

    #[test]
    // Mutation: split argv[0] on whitespace before applying the executable prefix rule.
    fn vrr_exclusions_process_name_like_mesa() {
        let tree = TempTree::new();
        let roots = roots(&tree);
        let proc_root = tree.path().join("proc");
        process(
            &proc_root,
            30,
            "/opt/x/firefox",
            "/opt/x/firefox --flag",
            &[],
        );
        process(
            &proc_root,
            31,
            "/home/u/.steam/ubuntu12_32/steam",
            "steam",
            &[],
        );
        process(&proc_root, 32, "/opt/x/other", "/usr/bin/unrelated", &[]);
        process(
            &proc_root,
            36,
            "/opt/x/firefox",
            "/opt/x/firefox-wrapper",
            &[],
        );
        process(&proc_root, 37, "/opt/My Game", "/opt/My Game --flag", &[]);
        write(
            roots
                .nvidia_driver_dir
                .join("nvidia-application-profiles-580-rc"),
            r#"{"rules":[{"pattern":"firefox","profile":["GLVRRAllowed",false]},{"pattern":"steam","profile":["GLVRRAllowed",false]},{"pattern":"unrelated","profile":["GLVRRAllowed",false]},{"pattern":"My Game","profile":["GLVRRAllowed",false]}]}"#,
        );
        let exclusions = VrrExclusions::load(&roots);

        assert_eq!(
            exclusions.excluded(30, &proc_root),
            Some(VrrExclusionSource::NvidiaProfiles)
        );
        assert_eq!(
            exclusions.excluded(31, &proc_root),
            Some(VrrExclusionSource::NvidiaProfiles)
        );
        assert_eq!(
            exclusions.excluded(32, &proc_root),
            Some(VrrExclusionSource::NvidiaProfiles)
        );
        assert_eq!(
            exclusions.excluded(36, &proc_root),
            Some(VrrExclusionSource::NvidiaProfiles)
        );
        assert_eq!(
            exclusions.excluded(37, &proc_root),
            Some(VrrExclusionSource::NvidiaProfiles)
        );
    }

    #[test]
    // Mutation: abort loading all sources if one NVIDIA directory is missing or a Mesa file is malformed.
    fn vrr_exclusions_bad_files() {
        let tree = TempTree::new();
        let first_roots = roots(&tree);
        let proc_root = tree.path().join("proc");
        write(
            first_roots.mesa_system_dir.join("10-broken.conf"),
            "<driconf><application>",
        );
        write(
            first_roots.mesa_etc_file.clone(),
            r#"<driconf><device><application executable="firefox"><option name="adaptive_sync" value="false"/></application></device></driconf>"#,
        );
        process(&proc_root, 33, "/usr/lib/firefox", "/usr/lib/firefox", &[]);

        assert_eq!(
            VrrExclusions::load(&first_roots).excluded(33, &proc_root),
            Some(VrrExclusionSource::MesaDriconf)
        );

        let nvidia_tree = TempTree::new();
        let nvidia_roots = roots(&nvidia_tree);
        let nvidia_proc = nvidia_tree.path().join("proc");
        write(
            nvidia_roots.mesa_etc_file.clone(),
            r#"<driconf><device><application executable="steam"><option name="adaptive_sync" value="false"/></application></device></driconf>"#,
        );
        process(&nvidia_proc, 34, "/usr/bin/steam", "/usr/bin/steam", &[]);
        assert_eq!(
            VrrExclusions::load(&nvidia_roots).excluded(34, &nvidia_proc),
            Some(VrrExclusionSource::MesaDriconf)
        );

        let comments_tree = TempTree::new();
        let comments_roots = roots(&comments_tree);
        let comments_proc = comments_tree.path().join("proc");
        write(
            comments_roots
                .nvidia_driver_dir
                .join("nvidia-application-profiles-580-rc"),
            "# Synthetic comment line\n{\"rules\":[{\"pattern\":\"steam\",\"profile\":[\"GLVRRAllowed\",false]}]}\n",
        );
        process(&comments_proc, 35, "/usr/bin/steam", "/usr/bin/steam", &[]);
        assert_eq!(
            VrrExclusions::load(&comments_roots).excluded(35, &comments_proc),
            Some(VrrExclusionSource::NvidiaProfiles)
        );
    }
}
