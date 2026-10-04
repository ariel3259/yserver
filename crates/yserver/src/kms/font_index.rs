//! X core font name matching, after libXfont2's fontfile FPE
//! (`src/fontfile/fontdir.c`, `fontfile.c`).
//!
//! Names are lowered once, when a font path element is read, and a
//! pattern once per request; matching is then a byte comparison with no
//! allocation. Lowering is libXfont's `ISOLatin1ToLower` on bytes, so `?`
//! matches one byte, as on Xorg.

/// libXfont's `ISOLatin1ToLower`: ASCII and the Latin-1 capitals.
pub(crate) fn latin1_lower(b: u8) -> u8 {
    match b {
        b'A'..=b'Z' | 0xC0..=0xD6 | 0xD8..=0xDE => b + 0x20,
        _ => b,
    }
}

/// A font name as the matcher sees it: lowered bytes and its dash count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LoweredName {
    pub(crate) bytes: Box<[u8]>,
    pub(crate) dashes: u32,
}

impl LoweredName {
    pub(crate) fn new(name: &str) -> Self {
        let bytes: Box<[u8]> = name.bytes().map(latin1_lower).collect();
        let dashes = count_dashes(&bytes);
        Self { bytes, dashes }
    }
}

fn count_dashes(b: &[u8]) -> u32 {
    u32::try_from(b.iter().filter(|&&c| c == b'-').count()).unwrap_or(u32::MAX)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tok {
    Star,
    Any,
    Lit(u8),
}

/// A client's font name pattern, lowered and tokenised once.
///
/// Semantics are Xorg's `PatternMatch`, including its quirk that the
/// character right after a `*` is compared literally: `*?` needs a `?` in
/// the name and `**` a `*`.
#[derive(Debug, Clone)]
pub(crate) struct FontPattern {
    toks: Vec<Tok>,
    dashes: u32,
}

impl FontPattern {
    pub(crate) fn new(pattern: &str) -> Self {
        let lowered: Vec<u8> = pattern.bytes().map(latin1_lower).collect();
        let mut toks = Vec::with_capacity(lowered.len());
        let mut it = lowered.iter().copied();
        while let Some(c) = it.next() {
            match c {
                b'*' => {
                    toks.push(Tok::Star);
                    if let Some(next) = it.next() {
                        toks.push(Tok::Lit(next));
                    }
                }
                b'?' => toks.push(Tok::Any),
                _ => toks.push(Tok::Lit(c)),
            }
        }
        Self {
            toks,
            dashes: count_dashes(&lowered),
        }
    }

    /// Whether the lowered `name` matches. Single pass with one backtrack
    /// point (the last `*`), so O(|pattern| x |name|) — the recursion in
    /// `PatternMatch` is exponential in the star count (#155).
    pub(crate) fn matches(&self, name: &LoweredName) -> bool {
        // Every `-` in the pattern is a literal that needs its own `-`.
        if name.dashes < self.dashes {
            return false;
        }
        let (t, n) = (&self.toks, &name.bytes[..]);
        let (mut pi, mut ni) = (0usize, 0usize);
        let mut star: Option<usize> = None;
        let mut star_ni = 0usize;
        while ni < n.len() {
            match t.get(pi) {
                Some(Tok::Any) => {
                    pi += 1;
                    ni += 1;
                    continue;
                }
                Some(&Tok::Lit(c)) if c == n[ni] => {
                    pi += 1;
                    ni += 1;
                    continue;
                }
                Some(Tok::Star) => {
                    star = Some(pi);
                    star_ni = ni;
                    pi += 1;
                    continue;
                }
                _ => {}
            }
            let Some(sp) = star else {
                return false;
            };
            pi = sp + 1;
            star_ni += 1;
            ni = star_ni;
        }
        while t.get(pi) == Some(&Tok::Star) {
            pi += 1;
        }
        pi == t.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(p: &str, n: &str) -> bool {
        FontPattern::new(p).matches(&LoweredName::new(n))
    }

    #[test]
    fn latin1_lowering_is_libxfonts() {
        assert_eq!(latin1_lower(b'Q'), b'q');
        assert_eq!(latin1_lower(0xC0), 0xE0);
        assert_eq!(latin1_lower(0xD7), 0xD7); // multiplication sign
        assert_eq!(latin1_lower(0xDE), 0xFE);
        assert_eq!(latin1_lower(0xDF), 0xDF);
        assert_eq!(latin1_lower(b'-'), b'-');
    }

    #[test]
    fn glob() {
        assert!(m("xtfont*", "xtfont0"));
        assert!(m("XTFONT0", "xtfont0"));
        assert!(m("xtfont0", "XTFONT0"));
        assert!(m(
            "-vsw-*-bold-r-*",
            "-vsw-testfont-bold-r-normal--13-130-75-75-m-70-iso8859-1"
        ));
        assert!(!m("xtfont?", "xtfont"));
        assert!(!m("nope", "xtfont0"));
        assert!(m("abc*", "abc"));
        assert!(m("*", ""));
        assert!(m("", ""));
        assert!(!m("", "a"));
        assert!(!m("?", ""));
        assert!(!m("abc", "abcd"));
        assert!(m("*b", "abab"));
        assert!(m("*ab", "aab"));
        assert!(m("a*b*c", "axxbyyc"));
        assert!(!m("*ab", "aba"));
        let name = "-misc-fixed-medium-r-normal--20-200-75-75-c-100-iso8859-1";
        assert!(m("-*-*-*-*-*-*-*-*-*-*-*-*-iso8859-1", name));
        assert!(!m("-*-*-*-*-*-*-*-*-*-*-*-*-iso10646-1", name));
        assert!(m("-*-*-*-*-*-*-*-*-*-*-*-*-*-*", name));
    }

    /// `PatternMatch` takes the character after a `*` literally (Xorg
    /// 21.1, measured in the font-list vng scenario).
    #[test]
    fn star_takes_the_next_character_literally() {
        assert!(!m("5x7**", "5x7"));
        assert!(m("5x7**", "5x7*"));
        assert!(!m("*?x7", "5x7"));
        assert!(m("*?x7", "a?x7"));
        assert!(m("5?7", "5x7"));
        assert!(m("5x7*", "5x7"));
    }

    /// `?` is one byte: a two-byte UTF-8 character needs two.
    #[test]
    fn any_is_one_byte() {
        assert!(!m("?", "é"));
        assert!(m("??", "é"));
        assert!(m("*é*", "xéy"));
    }

    /// The worst case for #155: an all-wildcard XLFD whose literal tail
    /// fails, so every star split is tried.
    #[test]
    fn glob_is_not_exponential() {
        let p = FontPattern::new("-*-*-*-*-*-*-*-*-*-*-*-*-iso8859-1");
        let n = LoweredName::new("-misc-fixed-medium-r-normal--20-200-75-75-c-100-iso10646-1");
        let start = std::time::Instant::now();
        for _ in 0..5000 {
            assert!(!p.matches(&n));
        }
        assert!(start.elapsed() < std::time::Duration::from_secs(3));
    }
}
