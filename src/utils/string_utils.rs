use unicode_normalization::UnicodeNormalization;

pub fn retirer_accents(s: &str) -> String {
    s.nfd()
        .filter(|c| !('\u{0300}'..='\u{036F}').contains(c))
        .collect()
}
#[cfg(test)]
mod tests {
    // Note this useful idiom: importing names from outer (for mod tests) scope.
    use super::*;

    #[test]
    fn test_accents() {
        assert_eq!(
            retirer_accents("écrire À ma mère où"),
            "ecrire A ma mere ou".to_string()
        );
    }
}
