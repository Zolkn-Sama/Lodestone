//! Extraction du texte brut d'un document (pdf / txt / md), puis normalisation.

use std::{panic, str::FromStr};

use crate::error::RagError;

/// Format d'un document source. Stocké en base dans `documents.source_type`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceType {
    Pdf,
    Txt,
    Md,
}

impl SourceType {
    pub fn as_str(&self) -> &'static str {
        match self {
            SourceType::Pdf => "pdf",
            SourceType::Txt => "txt",
            SourceType::Md => "md",
        }
    }

    /// Déduit le format de l'extension du nom de fichier (insensible à la casse).
    pub fn from_filename(name: &str) -> Result<Self, RagError> {
        let ext = name
            .rsplit_once('.')
            .map(|(_, ext)| ext)
            .ok_or_else(|| RagError::UnsupportedFormat(name.to_string()))?;
        ext.parse()
    }
}

impl FromStr for SourceType {
    type Err = RagError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "pdf" => Ok(SourceType::Pdf),
            "txt" | "text" => Ok(SourceType::Txt),
            "md" | "markdown" => Ok(SourceType::Md),
            other => Err(RagError::UnsupportedFormat(other.to_string())),
        }
    }
}

/// Extrait et normalise le texte d'un document. Le markdown est gardé tel quel :
/// sa syntaxe (titres, listes) reste lisible pour l'embedding et le LLM.
pub fn extract_text(bytes: &[u8], source: SourceType) -> Result<String, RagError> {
    let raw = match source {
        SourceType::Pdf => extract_pdf(bytes)?,
        SourceType::Txt | SourceType::Md => {
            let text = std::str::from_utf8(bytes).map_err(|_| RagError::InvalidUtf8)?;
            text.strip_prefix('\u{feff}').unwrap_or(text).to_string() // BOM UTF-8
        }
    };

    let text = normalize(&raw);
    if text.is_empty() {
        return Err(RagError::Empty);
    }
    Ok(text)
}

/// `pdf-extract` peut paniquer sur un PDF malformé : on convertit la panique
/// en erreur, pour qu'un mauvais fichier passe le document en `failed` au lieu
/// de tuer la tâche d'ingestion.
fn extract_pdf(bytes: &[u8]) -> Result<String, RagError> {
    match panic::catch_unwind(|| pdf_extract::extract_text_from_mem(bytes)) {
        Ok(Ok(text)) => Ok(text),
        Ok(Err(e)) => Err(RagError::Pdf(e.to_string())),
        Err(_) => Err(RagError::Pdf("structure du fichier invalide".into())),
    }
}

/// - fins de ligne unifiées en `\n` ;
/// - caractères NUL retirés (Postgres les refuse dans un TEXT) ;
/// - espaces de fin de ligne supprimés ;
/// - au plus une ligne vide entre deux paragraphes (`\n\n`), ce qui sert
///   ensuite de frontière de paragraphe au découpage.
pub fn normalize(raw: &str) -> String {
    let text = raw
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('\0', "");

    let mut out = String::with_capacity(text.len());
    let mut blank_lines = 0;
    for line in text.lines() {
        let line = line.trim_end();
        if line.trim().is_empty() {
            blank_lines += 1;
            continue;
        }
        if !out.is_empty() {
            out.push_str(if blank_lines > 0 { "\n\n" } else { "\n" });
        }
        out.push_str(line);
        blank_lines = 0;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_depuis_le_nom_de_fichier() {
        assert_eq!(
            SourceType::from_filename("RH.PDF").ok(),
            Some(SourceType::Pdf)
        );
        assert_eq!(
            SourceType::from_filename("notes.md").ok(),
            Some(SourceType::Md)
        );
        assert_eq!(
            SourceType::from_filename("a.b.txt").ok(),
            Some(SourceType::Txt)
        );
        assert!(SourceType::from_filename("contrat.docx").is_err());
        assert!(SourceType::from_filename("sans_extension").is_err());
    }

    #[test]
    fn aller_retour_texte() {
        for s in [SourceType::Pdf, SourceType::Txt, SourceType::Md] {
            assert_eq!(s.as_str().parse::<SourceType>().ok(), Some(s));
        }
    }

    #[test]
    fn txt_normalise() {
        let bytes = "\u{feff}Titre  \r\n\r\n\r\n\r\nLigne 1\r\nLigne\u{0}2\n\n".as_bytes();
        let text = extract_text(bytes, SourceType::Txt).expect("extraction");
        assert_eq!(text, "Titre\n\nLigne 1\nLigne2");
    }

    #[test]
    fn markdown_garde_sa_syntaxe() {
        let md = "# Congés\n\n- 25 jours\n- demande via RH\n";
        let text = extract_text(md.as_bytes(), SourceType::Md).expect("extraction");
        assert_eq!(text, "# Congés\n\n- 25 jours\n- demande via RH");
    }

    #[test]
    fn utf8_invalide_rejete() {
        let err = extract_text(&[0x66, 0x6f, 0xff, 0xfe], SourceType::Txt);
        assert!(matches!(err, Err(RagError::InvalidUtf8)));
    }

    #[test]
    fn document_vide_rejete() {
        let err = extract_text(b"  \n\n \r\n", SourceType::Txt);
        assert!(matches!(err, Err(RagError::Empty)));
    }

    #[test]
    fn pdf_corrompu_donne_une_erreur_sans_paniquer() {
        for bytes in [&b"pas un pdf"[..], b"%PDF-1.4\n1 0 obj\n<<", b""] {
            assert!(matches!(
                extract_text(bytes, SourceType::Pdf),
                Err(RagError::Pdf(_))
            ));
        }
    }

    #[test]
    fn pdf_valide() {
        let pdf = minimal_pdf("Procedure de conges");
        let text = extract_text(&pdf, SourceType::Pdf).expect("extraction");
        assert!(
            text.contains("Procedure de conges"),
            "texte extrait : {text:?}"
        );
    }

    /// Construit un PDF d'une page contenant `text` (police standard Helvetica),
    /// avec une table xref correcte. Évite de versionner un binaire de test.
    fn minimal_pdf(text: &str) -> Vec<u8> {
        let content = format!("BT /F1 12 Tf 72 720 Td ({text}) Tj ET");
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
             /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>"
                .to_string(),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica \
             /Encoding /WinAnsiEncoding >>"
                .to_string(),
            format!(
                "<< /Length {} >>\nstream\n{content}\nendstream",
                content.len()
            ),
        ];

        let mut pdf = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (i, obj) in objects.iter().enumerate() {
            offsets.push(pdf.len());
            pdf.extend(format!("{} 0 obj\n{obj}\nendobj\n", i + 1).as_bytes());
        }
        let xref = pdf.len();
        pdf.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
        for off in offsets {
            pdf.extend(format!("{off:010} 00000 n \n").as_bytes());
        }
        pdf.extend(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                objects.len() + 1
            )
            .as_bytes(),
        );
        pdf
    }
}
