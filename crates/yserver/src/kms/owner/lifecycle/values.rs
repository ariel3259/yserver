//! Value types shared by the pure lifecycle desired-state and arbitration layers.

use super::{LifecycleEpochId, LifecycleTransitionId};

/// Coordinator-assigned identity for one projected lifecycle event.
///
/// The coordinator owns allocation. Its checked counter starts at one and
/// reports exhaustion rather than wrapping or reusing an event identity.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct LifecycleEventId(u64);

impl LifecycleEventId {
    #[allow(dead_code)] // The coordinator consumes the allocator in Task 5.
    pub(crate) const fn first() -> Self {
        Self(1)
    }

    #[allow(dead_code)] // The coordinator consumes the allocator in Task 5.
    pub(crate) const fn get(self) -> u64 {
        self.0
    }

    pub(crate) const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    #[allow(dead_code)] // The coordinator consumes the allocator in Task 5.
    pub(crate) const fn checked_next(self) -> Option<Self> {
        match self.0.checked_add(1) {
            Some(next) => Some(Self(next)),
            None => None,
        }
    }

    #[allow(dead_code)] // The coordinator consumes the allocator in Task 5.
    pub(crate) fn next(self) -> Self {
        self.checked_next().expect("lifecycle event id exhausted")
    }
}

/// The C.0 `REC-4` lifecycle kinds, ordered from highest to lowest precedence.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum LifecycleKind {
    Shutdown,
    DeviceRemoved,
    VTRelease,
    DeviceAddedOrReplaced,
    VTAcquire,
    AdministrativeReprobe,
    IdentityChangingHotplug,
    TopologyRebuild,
    DPMS,
    NormalRecovery,
}

impl LifecycleKind {
    /// Every kind in C.0 precedence order, for exhaustive generated tables.
    pub const ALL: [Self; 10] = [
        Self::Shutdown,
        Self::DeviceRemoved,
        Self::VTRelease,
        Self::DeviceAddedOrReplaced,
        Self::VTAcquire,
        Self::AdministrativeReprobe,
        Self::IdentityChangingHotplug,
        Self::TopologyRebuild,
        Self::DPMS,
        Self::NormalRecovery,
    ];

    /// Zero is the highest precedence; larger values have lower precedence.
    pub const fn precedence(self) -> u8 {
        match self {
            Self::Shutdown => 0,
            Self::DeviceRemoved => 1,
            Self::VTRelease => 2,
            Self::DeviceAddedOrReplaced => 3,
            Self::VTAcquire => 4,
            Self::AdministrativeReprobe => 5,
            Self::IdentityChangingHotplug => 6,
            Self::TopologyRebuild => 7,
            Self::DPMS => 8,
            Self::NormalRecovery => 9,
        }
    }

    pub const fn outranks(self, other: Self) -> bool {
        self.precedence() < other.precedence()
    }
}

/// Terminal or pending outcome for one lifecycle event representative.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum Disposition {
    Applied(LifecycleTransitionId),
    AbsorbedByEvent(LifecycleEventId),
    AbsorbedByTransition(LifecycleTransitionId),
    Invalidated(InvalidationReason),
    SupersededBy(LifecycleEventId),
    Deferred(Prerequisite),
}

impl Disposition {
    /// One value for every variant, used by generated exhaustive tables.
    pub const ALL: [Self; 6] = [
        Self::Applied(LifecycleTransitionId::from_raw(1)),
        Self::AbsorbedByEvent(LifecycleEventId::from_raw(1)),
        Self::AbsorbedByTransition(LifecycleTransitionId::from_raw(1)),
        Self::Invalidated(InvalidationReason::Shutdown),
        Self::SupersededBy(LifecycleEventId::from_raw(1)),
        Self::Deferred(Prerequisite::SeatReleased),
    ];

    pub const fn is_terminal(self) -> bool {
        !matches!(self, Self::Deferred(_))
    }
}

/// C.0 invalidation causes, the four authorized `REC-6` external boundaries,
/// and the recovery-attempt failure outcome.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum InvalidationReason {
    Shutdown,
    DeviceRemoved,
    VTRelease,
    ProtocolOutputRemoved,
    NewerGeneration,
    DeviceAddedOrReplaced,
    VTAcquire,
    AdministrativeReprobe,
    IdentityChangingHotplug,
    RecoveryFailed,
}

impl InvalidationReason {
    /// Every invalidation reason, for exhaustive generated tables.
    pub const ALL: [Self; 10] = [
        Self::Shutdown,
        Self::DeviceRemoved,
        Self::VTRelease,
        Self::ProtocolOutputRemoved,
        Self::NewerGeneration,
        Self::DeviceAddedOrReplaced,
        Self::VTAcquire,
        Self::AdministrativeReprobe,
        Self::IdentityChangingHotplug,
        Self::RecoveryFailed,
    ];
}

/// External condition that currently prevents a desired target from running.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum Prerequisite {
    SeatReleased,
    DeviceAbsent,
    TopologyLatched(u64),
    ReadinessClosed,
}

impl Prerequisite {
    /// One value for every variant, used by generated exhaustive tables.
    pub const ALL: [Self; 4] = [
        Self::SeatReleased,
        Self::DeviceAbsent,
        Self::TopologyLatched(1),
        Self::ReadinessClosed,
    ];
}

/// Incident identity for one completion-loss recovery attempt.
///
/// This identity is allocated by the recovery incident owner and is not a
/// lifecycle transition identity.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct RecoveryId(u64);

impl RecoveryId {
    #[allow(dead_code)] // The incident allocator is added in Task 3.
    pub(crate) const fn first() -> Self {
        Self(1)
    }

    #[allow(dead_code)] // The incident allocator is added in Task 3.
    pub(crate) const fn get(self) -> u64 {
        self.0
    }

    pub(crate) const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    #[allow(dead_code)] // The incident allocator is added in Task 3.
    pub(crate) const fn checked_next(self) -> Option<Self> {
        match self.0.checked_add(1) {
            Some(next) => Some(Self(next)),
            None => None,
        }
    }

    #[allow(dead_code)] // The incident allocator is added in Task 3.
    pub(crate) fn next(self) -> Self {
        self.checked_next().expect("recovery id exhausted")
    }
}

/// The nine device lifecycle states from C.0 §6.4.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum DeviceLifecycleState {
    Unqualified,
    Ready,
    Quiescing,
    Poisoned,
    Recovering(RecoveryId),
    RecoveryFailed,
    ExecutorStalled,
    ShutdownExecutorStalled,
    Removed,
}

impl DeviceLifecycleState {
    /// One value for each state, used by exhaustive generated tables.
    pub const ALL: [Self; 9] = [
        Self::Unqualified,
        Self::Ready,
        Self::Quiescing,
        Self::Poisoned,
        Self::Recovering(RecoveryId::from_raw(1)),
        Self::RecoveryFailed,
        Self::ExecutorStalled,
        Self::ShutdownExecutorStalled,
        Self::Removed,
    ];
}

/// Ordinary or transition-owned work identity.
///
/// The caller supplies the opaque incarnation identity, keeping this pure
/// layer independent of device and renderer identity types.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub struct WorkTag<I> {
    pub incarnation: I,
    pub lifecycle_epoch: LifecycleEpochId,
    pub transition: Option<LifecycleTransitionId>,
}

impl<I> WorkTag<I> {
    pub const fn ordinary(incarnation: I, lifecycle_epoch: LifecycleEpochId) -> Self {
        Self {
            incarnation,
            lifecycle_epoch,
            transition: None,
        }
    }

    pub fn transition_owned(tag: TransitionTag<I>) -> Self {
        Self {
            incarnation: tag.incarnation,
            lifecycle_epoch: tag.lifecycle_epoch,
            transition: Some(tag.transition),
        }
    }
}

/// The transition-owned (`Some`) case of a [`WorkTag`].
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub struct TransitionTag<I> {
    pub incarnation: I,
    pub lifecycle_epoch: LifecycleEpochId,
    pub transition: LifecycleTransitionId,
}

impl<I> TransitionTag<I> {
    pub const fn new(
        incarnation: I,
        lifecycle_epoch: LifecycleEpochId,
        transition: LifecycleTransitionId,
    ) -> Self {
        Self {
            incarnation,
            lifecycle_epoch,
            transition,
        }
    }

    pub fn as_work_tag(self) -> WorkTag<I> {
        WorkTag::transition_owned(self)
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use super::{Disposition, LifecycleEventId, LifecycleKind};

    #[test]
    fn c0_3a_kind_order_is_c0_precedence() {
        let expected = [
            LifecycleKind::Shutdown,
            LifecycleKind::DeviceRemoved,
            LifecycleKind::VTRelease,
            LifecycleKind::DeviceAddedOrReplaced,
            LifecycleKind::VTAcquire,
            LifecycleKind::AdministrativeReprobe,
            LifecycleKind::IdentityChangingHotplug,
            LifecycleKind::TopologyRebuild,
            LifecycleKind::DPMS,
            LifecycleKind::NormalRecovery,
        ];

        assert_eq!(LifecycleKind::ALL, expected);
        for (rank, kind) in LifecycleKind::ALL.into_iter().enumerate() {
            assert_eq!(kind.precedence(), rank as u8);
        }
        for adjacent in LifecycleKind::ALL.windows(2) {
            assert!(adjacent[0] < adjacent[1]);
            assert!(adjacent[0].outranks(adjacent[1]));
        }
    }

    #[test]
    fn c0_3a_event_id_is_checked() {
        let first = LifecycleEventId::first();
        assert_eq!(first.get(), 1);
        assert_eq!(first.checked_next(), Some(first.next()));
        assert_eq!(first.next().get(), 2);
        assert_eq!(LifecycleEventId::from_raw(u64::MAX).checked_next(), None);
    }

    #[test]
    fn c0_3a_terminal_and_deferred_are_distinct() {
        let expected_terminal = [true, true, true, true, true, false];
        assert_eq!(
            Disposition::ALL.map(Disposition::is_terminal),
            expected_terminal
        );
    }

    #[test]
    fn c0_3a_layer_imports_nothing_effectful() {
        fn rust_sources(path: &Path, found: &mut Vec<(std::path::PathBuf, String)>) {
            for entry in fs::read_dir(path).expect("read lifecycle source directory") {
                let entry = entry.expect("read lifecycle source entry");
                let path = entry.path();
                if path.is_dir() {
                    rust_sources(&path, found);
                } else if path.extension().is_some_and(|extension| extension == "rs") {
                    let source = fs::read_to_string(&path).expect("read lifecycle Rust source");
                    found.push((path, source));
                }
            }
        }

        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/kms/owner/lifecycle");
        let mut sources = Vec::new();
        rust_sources(&root, &mut sources);

        fn source_module_path(root: &Path, source_path: &Path) -> Vec<String> {
            let relative = source_path
                .strip_prefix(root)
                .expect("lifecycle source is under its root");
            let mut module_path =
                vec!["kms".to_owned(), "owner".to_owned(), "lifecycle".to_owned()];
            for component in relative.components() {
                let component = component.as_os_str().to_string_lossy();
                if component.ends_with(".rs") {
                    let name = component.trim_end_matches(".rs");
                    if name != "mod" {
                        module_path.push(name.to_owned());
                    }
                } else {
                    module_path.push(component.into_owned());
                }
            }
            module_path
        }

        // Keep Rust identifiers and `::` tokens, while treating whitespace,
        // line breaks, and comments as trivia. String and raw-string contents
        // are omitted so prose or diagnostics do not look like imports.
        fn rust_path_tokens(source: &str) -> Vec<String> {
            fn raw_string_end(bytes: &[u8], start: usize) -> Option<usize> {
                let mut quote = start + 1;
                while quote < bytes.len() && bytes[quote] == b'#' {
                    quote += 1;
                }
                if bytes.get(quote) != Some(&b'"') {
                    return None;
                }
                let hashes = quote - start - 1;
                let mut index = quote + 1;
                while index < bytes.len() {
                    if bytes[index] == b'"'
                        && bytes[index + 1..].len() >= hashes
                        && bytes[index + 1..index + 1 + hashes]
                            .iter()
                            .all(|byte| *byte == b'#')
                    {
                        return Some(index + hashes + 1);
                    }
                    index += 1;
                }
                Some(bytes.len())
            }

            let bytes = source.as_bytes();
            let mut tokens = Vec::new();
            let mut index = 0;
            while index < bytes.len() {
                if bytes[index].is_ascii_whitespace() {
                    index += 1;
                } else if bytes[index..].starts_with(b"//") {
                    index += 2;
                    while index < bytes.len() && bytes[index] != b'\n' {
                        index += 1;
                    }
                } else if bytes[index..].starts_with(b"/*") {
                    index += 2;
                    let mut depth = 1usize;
                    while index < bytes.len() && depth > 0 {
                        if bytes[index..].starts_with(b"/*") {
                            depth += 1;
                            index += 2;
                        } else if bytes[index..].starts_with(b"*/") {
                            depth -= 1;
                            index += 2;
                        } else {
                            index += 1;
                        }
                    }
                } else if bytes[index] == b'b'
                    && bytes.get(index + 1) == Some(&b'r')
                    && bytes
                        .get(index + 2)
                        .is_some_and(|byte| *byte == b'"' || *byte == b'#')
                {
                    index =
                        raw_string_end(bytes, index + 1).expect("raw byte string starts with r");
                } else if bytes[index] == b'r'
                    && bytes.get(index + 1).is_some_and(|byte| *byte == b'#')
                    && bytes
                        .get(index + 2)
                        .is_some_and(|byte| byte.is_ascii_alphabetic() || *byte == b'_')
                {
                    let start = index + 2;
                    index = start + 1;
                    while index < bytes.len()
                        && (bytes[index].is_ascii_alphanumeric() || bytes[index] == b'_')
                    {
                        index += 1;
                    }
                    tokens.push(source[start..index].to_owned());
                } else if bytes[index] == b'r'
                    && (index + 1 < bytes.len()
                        && (bytes[index + 1] == b'"' || bytes[index + 1] == b'#'))
                {
                    if let Some(end) = raw_string_end(bytes, index) {
                        index = end;
                    } else {
                        tokens.push("r".to_owned());
                        index += 1;
                    }
                } else if bytes[index] == b'"' {
                    index += 1;
                    while index < bytes.len() {
                        if bytes[index] == b'\\' {
                            index = (index + 2).min(bytes.len());
                        } else if bytes[index] == b'"' {
                            index += 1;
                            break;
                        } else {
                            index += 1;
                        }
                    }
                } else if bytes[index] == b':' && bytes.get(index + 1) == Some(&b':') {
                    tokens.push("::".to_owned());
                    index += 2;
                } else if bytes[index].is_ascii_alphabetic() || bytes[index] == b'_' {
                    let start = index;
                    index += 1;
                    while index < bytes.len()
                        && (bytes[index].is_ascii_alphanumeric() || bytes[index] == b'_')
                    {
                        index += 1;
                    }
                    tokens.push(source[start..index].to_owned());
                } else {
                    tokens.push((bytes[index] as char).to_string());
                    index += 1;
                }
            }
            tokens
        }

        fn is_effectful_path(path: &[String]) -> bool {
            let targets: [&[&str]; 4] = [
                &["kms", "render"],
                &["kms", "executor"],
                &["drm"],
                &["platform"],
            ];
            let resolved_modules = if path.first().is_some_and(|segment| segment == "crate") {
                &path[1..]
            } else {
                path
            };
            targets.iter().any(|target| {
                resolved_modules.len() >= target.len()
                    && resolved_modules[..target.len()]
                        .iter()
                        .map(String::as_str)
                        .eq(target.iter().copied())
            })
        }

        fn resolve_path(
            tokens: &[String],
            start: usize,
            module_path: &[String],
            inherited: Option<&[String]>,
            found: &mut Option<Vec<String>>,
        ) -> Option<Vec<String>> {
            let first = tokens.get(start)?;
            let (mut path, mut index) = match first.as_str() {
                "crate" => (vec!["crate".to_owned()], start + 1),
                "self" => {
                    let mut path = module_path.to_vec();
                    let mut index = start + 1;
                    while tokens.get(index).is_some_and(|token| token == "::")
                        && tokens.get(index + 1).is_some_and(|token| token == "super")
                    {
                        path.pop()?;
                        index += 2;
                    }
                    (path, index)
                }
                "super" => {
                    let mut path = module_path.to_vec();
                    let mut index = start;
                    while tokens.get(index).is_some_and(|token| token == "super") {
                        path.pop()?;
                        index += 1;
                        if tokens.get(index).is_some_and(|token| token == "::")
                            && tokens.get(index + 1).is_some_and(|token| token == "super")
                        {
                            index += 1;
                        } else {
                            break;
                        }
                    }
                    (path, index)
                }
                identifier
                    if identifier
                        .as_bytes()
                        .first()
                        .is_some_and(u8::is_ascii_alphabetic) =>
                {
                    let mut path = inherited?.to_vec();
                    path.push(identifier.to_owned());
                    (path, start + 1)
                }
                _ => return None,
            };
            if is_effectful_path(&path) {
                *found = Some(path.clone());
            }

            while tokens.get(index).is_some_and(|token| token == "::") {
                match tokens.get(index + 1).map(String::as_str) {
                    Some("super") => {
                        path.pop()?;
                        index += 2;
                    }
                    Some("{") => {
                        // A use-tree group carries its parent path into each
                        // child, e.g. `crate::kms::{render, executor}`.
                        let mut depth = 1usize;
                        let mut child_start = index + 2;
                        let mut cursor = child_start;
                        while cursor < tokens.len() && depth > 0 {
                            match tokens[cursor].as_str() {
                                "{" => depth += 1,
                                "}" => {
                                    depth -= 1;
                                    if depth == 0 {
                                        if child_start < cursor {
                                            let _ = resolve_path(
                                                tokens,
                                                child_start,
                                                &path,
                                                Some(&path),
                                                found,
                                            );
                                        }
                                        return Some(path);
                                    }
                                }
                                "," if depth == 1 => {
                                    if child_start < cursor {
                                        let _ = resolve_path(
                                            tokens,
                                            child_start,
                                            &path,
                                            Some(&path),
                                            found,
                                        );
                                    }
                                    child_start = cursor + 1;
                                }
                                _ => {}
                            }
                            cursor += 1;
                        }
                        return Some(path);
                    }
                    Some(segment)
                        if segment
                            .as_bytes()
                            .first()
                            .is_some_and(u8::is_ascii_alphabetic)
                            || segment == "_" =>
                    {
                        path.push(segment.to_owned());
                        if is_effectful_path(&path) {
                            *found = Some(path.clone());
                        }
                        index += 2;
                    }
                    _ => break,
                }
            }
            Some(path)
        }

        fn find_effectful_reference(
            tokens: &[String],
            module_path: &[String],
        ) -> Option<Vec<String>> {
            let mut path_at_token = Vec::with_capacity(tokens.len());
            let mut current_module = module_path.to_vec();
            let mut brace_modules = Vec::new();
            for (index, token) in tokens.iter().enumerate() {
                path_at_token.push(current_module.clone());
                match token.as_str() {
                    "{" => {
                        let module_name = (index >= 2 && tokens[index - 2] == "mod")
                            .then(|| tokens[index - 1].clone());
                        if let Some(name) = &module_name {
                            current_module.push(name.clone());
                        }
                        brace_modules.push(module_name);
                    }
                    "}" if brace_modules.pop().flatten().is_some() => {
                        current_module.pop();
                    }
                    "}" => {}
                    _ => {}
                }
            }

            let mut found = None;
            for (index, token) in tokens.iter().enumerate() {
                if matches!(token.as_str(), "crate" | "self" | "super") {
                    let _ = resolve_path(tokens, index, &path_at_token[index], None, &mut found);
                }
            }
            found
        }

        for (source_path, source) in sources {
            let module_path = source_module_path(&root, &source_path);
            let tokens = rust_path_tokens(&source);
            let forbidden = find_effectful_reference(&tokens, &module_path);
            assert!(
                forbidden.is_none(),
                "lifecycle source {} references an effectful module via {:?}",
                source_path.display(),
                forbidden,
            );
        }
    }
}
