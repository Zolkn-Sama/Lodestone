use std::{convert::Infallible, net::IpAddr, net::SocketAddr};

use axum::{
    extract::{ConnectInfo, FromRequestParts},
    http::{HeaderMap, request::Parts},
};

use crate::state::AppState;

/// IP du client, pour limiter les tentatives par adresse. `None` si inconnue
/// (aucune limite par IP n'est alors appliquée).
pub struct ClientIp(pub Option<IpAddr>);

impl FromRequestParts<AppState> for ClientIp {
    type Rejection = Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let peer = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|c| c.0.ip());
        Ok(ClientIp(client_ip(
            &parts.headers,
            state.config.trust_proxy_headers,
            peer,
        )))
    }
}

/// Derrière un reverse proxy, toutes les connexions viennent du proxy : il
/// faut lire `X-Forwarded-For` (si `trust_proxy`). On prend la DERNIÈRE
/// adresse, celle ajoutée par notre proxy : les précédentes viennent du client
/// et peuvent être forgées.
fn client_ip(headers: &HeaderMap, trust_proxy: bool, peer: Option<IpAddr>) -> Option<IpAddr> {
    if trust_proxy
        && let Some(ip) = headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.rsplit(',').next())
            .and_then(|last| last.trim().parse().ok())
    {
        return Some(ip);
    }
    peer
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(xff: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert("x-forwarded-for", xff.parse().expect("en-tête"));
        h
    }

    #[test]
    fn sans_proxy_l_adresse_de_connexion() {
        let peer: IpAddr = "10.0.0.1".parse().expect("ip");
        // En-tête ignoré si le proxy n'est pas de confiance (sinon : usurpation triviale).
        assert_eq!(
            client_ip(&headers("1.2.3.4"), false, Some(peer)),
            Some(peer)
        );
        assert_eq!(client_ip(&HeaderMap::new(), false, None), None);
    }

    #[test]
    fn derriere_un_proxy_la_derniere_adresse() {
        let proxy: IpAddr = "127.0.0.1".parse().expect("ip");
        assert_eq!(
            client_ip(&headers("6.6.6.6, 203.0.113.7"), true, Some(proxy)),
            Some("203.0.113.7".parse().expect("ip")),
            "la première adresse (forgeable) est ignorée"
        );
        assert_eq!(
            client_ip(&headers("pas-une-ip"), true, Some(proxy)),
            Some(proxy),
            "en-tête illisible : adresse de connexion"
        );
    }
}
