//! Bus d'événements en mémoire : les services publient, les connexions
//! WebSocket relaient aux membres de l'org concernée.

use tokio::sync::broadcast;
use uuid::Uuid;

use crate::ws::protocol::ServerMsg;

/// Événements en attente par abonné ; au-delà, un abonné trop lent perd les
/// plus anciens (il est prévenu par `RecvError::Lagged`).
const CAPACITY: usize = 256;

/// Un message destiné aux membres d'une org.
#[derive(Debug, Clone)]
pub struct OrgEvent {
    pub org_id: Uuid,
    pub message: ServerMsg,
}

#[derive(Clone)]
pub struct Events {
    tx: broadcast::Sender<OrgEvent>,
}

impl Events {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(CAPACITY);
        Self { tx }
    }

    /// Publie sans attendre ; sans abonné, l'événement est simplement perdu.
    pub fn publish(&self, org_id: Uuid, message: ServerMsg) {
        let _ = self.tx.send(OrgEvent { org_id, message });
    }

    pub fn subscribe(&self) -> broadcast::Receiver<OrgEvent> {
        self.tx.subscribe()
    }
}

impl Default for Events {
    fn default() -> Self {
        Self::new()
    }
}
