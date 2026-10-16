//! Découpage du texte en fragments à embedder.
//!
//! Pas de tokenizer : on estime 1 token ≈ 4 caractères (ordre de grandeur
//! correct en français comme en anglais pour `nomic-embed-text`), ce qui garde
//! le découpage pur, rapide et déterministe.
//!
//! Le texte est découpé aux frontières les plus « naturelles » possibles :
//! paragraphes (`\n\n`), puis lignes, puis phrases, puis mots, et en dernier
//! recours caractères. Les morceaux sont ensuite regroupés jusqu'à
//! `max_tokens`, chaque fragment reprenant la fin du précédent
//! (`overlap_tokens`) pour ne pas perdre le contexte à la coupure.

/// Paramètres du découpage. Par défaut : fragments de 800 tokens maximum,
/// chevauchement de 80 tokens.
#[derive(Debug, Clone, Copy)]
pub struct ChunkConfig {
    pub max_tokens: usize,
    pub overlap_tokens: usize,
}

impl Default for ChunkConfig {
    fn default() -> Self {
        Self {
            max_tokens: 800,
            overlap_tokens: 80,
        }
    }
}

/// Un fragment, numéroté dans l'ordre du document (`document_chunks.chunk_index`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub index: usize,
    pub content: String,
}

const CHARS_PER_TOKEN: usize = 4;

pub fn estimate_tokens(text: &str) -> usize {
    text.chars().count().div_ceil(CHARS_PER_TOKEN)
}

/// Découpe un texte (déjà normalisé par `extract`) en fragments.
pub fn chunk_text(text: &str, config: &ChunkConfig) -> Vec<Chunk> {
    let max = config.max_tokens.max(2);
    // Le chevauchement ne peut pas manger tout le fragment.
    let overlap = config.overlap_tokens.min(max / 2);
    // Place pour le contenu neuf : max - chevauchement - 1 (le séparateur).
    let body_limit = (max - overlap).saturating_sub(1).max(1);

    let mut units = Vec::new();
    push_units(text.trim(), Level::Paragraph, "", body_limit, &mut units);

    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut has_body = false; // `current` contient-il autre chose que le chevauchement ?

    for unit in units {
        if has_body {
            let candidate =
                estimate_tokens(&current) + estimate_tokens(unit.sep) + estimate_tokens(unit.text);
            if candidate > max {
                let tail = tail_words(&current, overlap);
                chunks.push(std::mem::replace(&mut current, tail));
            }
        }
        if !current.is_empty() {
            current.push_str(unit.sep);
        }
        current.push_str(unit.text);
        has_body = true;
    }
    if has_body {
        chunks.push(current);
    }

    chunks
        .into_iter()
        .enumerate()
        .map(|(index, content)| Chunk { index, content })
        .collect()
}

/// Un morceau insécable, avec le séparateur qui le précédait dans le texte.
struct Unit<'a> {
    text: &'a str,
    sep: &'static str,
}

#[derive(Clone, Copy)]
enum Level {
    Paragraph,
    Line,
    Sentence,
    Word,
    Char,
}

impl Level {
    fn next(self) -> Self {
        match self {
            Level::Paragraph => Level::Line,
            Level::Line => Level::Sentence,
            Level::Sentence => Level::Word,
            Level::Word | Level::Char => Level::Char,
        }
    }
}

/// Ajoute `text` comme une seule unité s'il tient dans `limit`, sinon le
/// redécoupe au niveau suivant (paragraphe -> ligne -> phrase -> mot -> caractère).
fn push_units<'a>(
    text: &'a str,
    level: Level,
    sep: &'static str,
    limit: usize,
    out: &mut Vec<Unit<'a>>,
) {
    if text.is_empty() {
        return;
    }
    if estimate_tokens(text) <= limit {
        out.push(Unit { text, sep });
        return;
    }

    let (pieces, joiner): (Vec<&str>, &'static str) = match level {
        Level::Paragraph => (text.split("\n\n").collect(), "\n\n"),
        Level::Line => (text.split('\n').collect(), "\n"),
        Level::Sentence => (split_sentences(text), " "),
        Level::Word => (text.split_whitespace().collect(), " "),
        Level::Char => (split_chars(text, limit * CHARS_PER_TOKEN), ""),
    };

    let mut first = true;
    for piece in pieces.into_iter().map(str::trim).filter(|p| !p.is_empty()) {
        let piece_sep = if first { sep } else { joiner };
        first = false;
        push_units(piece, level.next(), piece_sep, limit, out);
    }
}

/// Coupe après `.`, `!`, `?` ou `…` suivis d'un espace.
fn split_sentences(text: &str) -> Vec<&str> {
    let mut pieces = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((_, c)) = chars.next() {
        if matches!(c, '.' | '!' | '?' | '…')
            && let Some(&(next_i, next_c)) = chars.peek()
            && next_c.is_whitespace()
        {
            pieces.push(&text[start..next_i]);
            start = next_i;
        }
    }
    pieces.push(&text[start..]);
    pieces
}

/// Dernier recours (ex. un « mot » de 10 000 caractères) : coupe tous les
/// `max_chars` caractères, sans jamais couper un caractère UTF-8 en deux.
fn split_chars(text: &str, max_chars: usize) -> Vec<&str> {
    let max_chars = max_chars.max(1);
    let mut pieces = Vec::new();
    let mut start = 0;
    for (count, (i, _)) in text.char_indices().enumerate() {
        if count > 0 && count % max_chars == 0 {
            pieces.push(&text[start..i]);
            start = i;
        }
    }
    pieces.push(&text[start..]);
    pieces
}

/// Les derniers mots de `text`, dans la limite de `overlap_tokens`.
fn tail_words(text: &str, overlap_tokens: usize) -> String {
    let budget = overlap_tokens * CHARS_PER_TOKEN;
    let mut used = 0;
    let mut words = Vec::new();
    for word in text.split_whitespace().rev() {
        let cost = word.chars().count() + usize::from(!words.is_empty());
        if used + cost > budget {
            break;
        }
        used += cost;
        words.push(word);
    }
    words.reverse();
    words.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Paragraphe d'environ `tokens` tokens, reconnaissable grâce à `id`.
    fn paragraph(id: usize, tokens: usize) -> String {
        let mut p = format!("Paragraphe{id}.");
        let mut n = 0;
        while estimate_tokens(&p) < tokens {
            p.push_str(&format!(" mot{id}x{n}"));
            n += 1;
        }
        p
    }

    fn words(s: &str) -> Vec<&str> {
        s.split_whitespace().collect()
    }

    #[test]
    fn texte_vide_aucun_fragment() {
        assert!(chunk_text("", &ChunkConfig::default()).is_empty());
        assert!(chunk_text(" \n\n ", &ChunkConfig::default()).is_empty());
    }

    #[test]
    fn texte_court_un_seul_fragment_intact() {
        let text = "# Congés\n\nOn a droit à 25 jours.\nDemande via le portail RH.";
        let chunks = chunk_text(text, &ChunkConfig::default());
        assert_eq!(
            chunks,
            vec![Chunk {
                index: 0,
                content: text.to_string()
            }]
        );
    }

    #[test]
    fn taille_maximale_respectee() {
        let text: Vec<String> = (0..40).map(|i| paragraph(i, 50 + i * 13)).collect();
        let config = ChunkConfig::default();
        let chunks = chunk_text(&text.join("\n\n"), &config);
        assert!(chunks.len() > 1);
        for c in &chunks {
            let tokens = estimate_tokens(&c.content);
            assert!(
                tokens <= config.max_tokens,
                "fragment {} : {tokens} tokens",
                c.index
            );
        }
    }

    #[test]
    fn fragments_bien_remplis() {
        // 20 paragraphes de ~100 tokens : on remplit jusqu'à ~800, pas 100 par fragment.
        let text: Vec<String> = (0..20).map(|i| paragraph(i, 100)).collect();
        let chunks = chunk_text(&text.join("\n\n"), &ChunkConfig::default());
        let (last, full) = chunks.split_last().expect("au moins un fragment");
        for c in full {
            assert!(
                estimate_tokens(&c.content) >= 500,
                "fragment {} trop petit",
                c.index
            );
        }
        assert!(!last.content.is_empty());
    }

    #[test]
    fn paragraphes_jamais_coupes_quand_ils_tiennent() {
        let paragraphs: Vec<String> = (0..20).map(|i| paragraph(i, 100)).collect();
        let chunks = chunk_text(&paragraphs.join("\n\n"), &ChunkConfig::default());
        for p in &paragraphs {
            assert!(
                chunks.iter().any(|c| c.content.contains(p.as_str())),
                "paragraphe coupé : {}",
                &p[..20]
            );
        }
    }

    #[test]
    fn chevauchement_entre_fragments() {
        let text: Vec<String> = (0..20).map(|i| paragraph(i, 100)).collect();
        let chunks = chunk_text(&text.join("\n\n"), &ChunkConfig::default());
        assert!(chunks.len() > 1);
        for pair in chunks.windows(2) {
            let prev = words(&pair[0].content);
            let next = words(&pair[1].content);
            let overlap = tail_words(&pair[0].content, 80);
            let overlap = words(&overlap);

            // Le fragment suivant commence par la fin du précédent…
            assert!(
                overlap.len() >= 10,
                "fragment {} : chevauchement trop court",
                pair[1].index
            );
            assert!(prev.ends_with(&overlap), "fragment {}", pair[0].index);
            assert!(next.starts_with(&overlap), "fragment {}", pair[1].index);
            // … dans la limite de 80 tokens.
            assert!(estimate_tokens(&overlap.join(" ")) <= 80);
        }
    }

    #[test]
    fn aucun_mot_perdu() {
        let text: Vec<String> = (0..30).map(|i| paragraph(i, 30 + i * 37)).collect();
        let text = text.join("\n\n");
        let chunks = chunk_text(&text, &ChunkConfig::default());
        let all: std::collections::HashSet<&str> =
            chunks.iter().flat_map(|c| words(&c.content)).collect();
        for w in words(&text) {
            assert!(all.contains(w), "mot perdu : {w}");
        }
    }

    #[test]
    fn indices_consecutifs() {
        let text: Vec<String> = (0..20).map(|i| paragraph(i, 150)).collect();
        let chunks = chunk_text(&text.join("\n\n"), &ChunkConfig::default());
        for (i, c) in chunks.iter().enumerate() {
            assert_eq!(c.index, i);
        }
    }

    #[test]
    fn paragraphe_geant_coupe_aux_phrases() {
        // Un seul paragraphe de ~3000 tokens, fait de phrases de ~25 tokens.
        let sentence = "Le salarié transmet sa demande de congés au manager pour validation.";
        let text = vec![sentence; 170].join(" ");
        let chunks = chunk_text(&text, &ChunkConfig::default());
        assert!(chunks.len() >= 4);
        for c in &chunks {
            assert!(estimate_tokens(&c.content) <= 800);
            // Coupé en fin de phrase : chaque fragment se termine par un point.
            assert!(
                c.content.ends_with('.'),
                "fragment {} : …{}",
                c.index,
                &c.content[c.content.len() - 20..]
            );
        }
    }

    #[test]
    fn mot_geant_et_utf8_sans_panique() {
        // 5000 caractères multi-octets sans aucun espace.
        let text = "é€😀".repeat(1700);
        let chunks = chunk_text(&text, &ChunkConfig::default());
        assert!(chunks.len() > 1);
        let total: usize = chunks.iter().map(|c| c.content.chars().count()).sum();
        assert_eq!(
            total,
            text.chars().count(),
            "aucun caractère perdu (pas de chevauchement possible sans espace)"
        );
        for c in &chunks {
            assert!(estimate_tokens(&c.content) <= 800);
        }
    }

    #[test]
    fn configuration_personnalisee() {
        let text: Vec<String> = (0..10).map(|i| paragraph(i, 40)).collect();
        let config = ChunkConfig {
            max_tokens: 100,
            overlap_tokens: 10,
        };
        let chunks = chunk_text(&text.join("\n\n"), &config);
        assert!(chunks.len() >= 5);
        for c in &chunks {
            assert!(estimate_tokens(&c.content) <= 100);
        }
    }

    #[test]
    fn deterministe() {
        let text: Vec<String> = (0..15).map(|i| paragraph(i, 120)).collect();
        let text = text.join("\n\n");
        let config = ChunkConfig::default();
        assert_eq!(chunk_text(&text, &config), chunk_text(&text, &config));
    }
}
