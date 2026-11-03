/// Returns true when a username contains a known hateful or racial slur.
///
/// Usernames are normalized before checking so separators and common leetspeak
/// substitutions cannot be used to trivially bypass the registration filter.
pub fn is_disallowed_username(username: &str) -> bool {
    let normalized: String = username
        .chars()
        .flat_map(|character| {
            let character = character.to_ascii_lowercase();
            let replacement = match character {
                '0' => 'o',
                '1' => 'i',
                '3' => 'e',
                '4' => 'a',
                '5' => 's',
                '7' => 't',
                _ => character,
            };
            replacement.is_ascii_alphanumeric().then_some(replacement)
        })
        .collect();

    const BLOCKED_TERMS: &[&str] = &[
        "nigger",
        "nigga",
        "coon",
        "spic",
        "kike",
        "chink",
        "gook",
        "wetback",
        "sandnigger",
        "heilhitler",
        "whitepower",
        "blackpower",
    ];

    BLOCKED_TERMS
        .iter()
        .any(|term| normalized.contains(term))
}
