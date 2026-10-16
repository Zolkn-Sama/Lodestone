//! lodestone-rag : partie pure du pipeline RAG (§8), sans réseau ni base.
//!
//! `extract` (pdf / txt / md -> texte normalisé) puis `chunk` (texte -> fragments).
//! L'embedding passe par `lodestone-llm`, l'insertion par le backend.

pub mod chunk;
pub mod error;
pub mod extract;

pub use chunk::{Chunk, ChunkConfig, chunk_text};
pub use error::RagError;
pub use extract::{SourceType, extract_text};
