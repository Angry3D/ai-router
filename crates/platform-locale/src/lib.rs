//! Safe macOS preferred-language sampling for the application language
//! authority.
//!
//! `core-foundation` 0.9 does not wrap `CFLocale`, and the workspace forbids
//! `unsafe` in every other crate, so this crate exists solely to expose one
//! safe API for `CFLocaleCopyPreferredLanguages`. The Copy-returned array is
//! released deterministically through `CFArray`'s RAII wrapper.

use core_foundation::{
    array::{CFArray, CFArrayRef},
    base::TCFType,
    string::CFString,
};

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    /// Returns the user's preferred localizations, most preferred first.
    /// Copy semantics: the caller owns the returned array; it may be NULL.
    fn CFLocaleCopyPreferredLanguages() -> CFArrayRef;
}

/// Samples the first macOS preferred-language entry.
///
/// Returns `None` when the API is unavailable or the list is empty; callers
/// resolve that case to English through `AppLocale::from_platform_tag`.
// SAFETY boundaries are documented inline; the unsafe surface is this
// function's body plus the extern declaration above and nothing else.
#[allow(unsafe_code)]
pub fn preferred_language_tag() -> Option<String> {
    // SAFETY: the function takes no arguments. A non-NULL return is a valid
    // CFArray of CFString values; `wrap_under_create_rule` adopts the
    // Copy-returned reference so its Drop releases the array deterministically.
    let pointer = unsafe { CFLocaleCopyPreferredLanguages() };
    if pointer.is_null() {
        return None;
    }
    let languages = unsafe { CFArray::<CFString>::wrap_under_create_rule(pointer) };
    // SAFETY: `get` bounds-checks the index and wraps the array element as a
    // borrowed `CFString` without taking ownership.
    languages.get(0).map(|tag| tag.to_string())
}

#[cfg(test)]
mod tests {
    use super::preferred_language_tag;

    /// Real native-adapter observation on the host macOS: CFLocale documents
    /// at least one entry (falling back to a default) once the session is up.
    /// Synthetic normalization coverage lives in `router-core::domain`.
    #[test]
    fn preferred_language_tag_returns_a_nonempty_entry_on_macos() {
        let tag = preferred_language_tag();
        assert!(
            tag.as_deref().is_some_and(|tag| !tag.is_empty()),
            "CFLocaleCopyPreferredLanguages returned no usable entry"
        );
    }
}
