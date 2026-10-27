//! Container readiness without a shell, extra HTTP utility, or business keys.
use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    time::Duration,
};

fn target(bind: &str) -> Result<reqwest::Url, Box<dyn std::error::Error>> {
    let port = bind
        .rsplit_once(':')
        .and_then(|(_, port)| port.parse::<u16>().ok())
        .filter(|port| *port != 0)
        .ok_or("Readiness probe requires an explicit listening port")?;
    let authority = if let Ok(mut address) = bind.parse::<SocketAddr>() {
        if address.ip().is_unspecified() {
            address.set_ip(match address.ip() {
                IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::LOCALHOST),
                IpAddr::V6(_) => IpAddr::V6(Ipv6Addr::LOCALHOST),
            });
        }
        address.to_string()
    } else {
        bind.to_owned()
    };
    let url = reqwest::Url::parse(&format!("http://{authority}/ready"))?;
    if !url.username().is_empty()
        || url.password().is_some()
        || url.port_or_known_default() != Some(port)
        || url.path() != "/ready"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("Readiness probe requires a host and explicit listening port".into());
    }
    Ok(url)
}

pub async fn check(bind: &str) -> Result<(), Box<dyn std::error::Error>> {
    let response = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_millis(500))
        .timeout(Duration::from_millis(1500))
        .build()?
        .get(target(bind)?)
        .send()
        .await?;
    if response.status() != reqwest::StatusCode::OK {
        return Err("Freight storage is not ready".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };
    #[test]
    fn wildcard_bind_targets_are_local_and_authority_cannot_inject_credentials_or_paths() {
        assert_eq!(
            target("0.0.0.0:80").expect("default HTTP port").as_str(),
            "http://127.0.0.1/ready"
        );
        assert_eq!(
            target("0.0.0.0:8090").expect("IPv4").as_str(),
            "http://127.0.0.1:8090/ready"
        );
        assert_eq!(
            target("[::]:8090").expect("IPv6").as_str(),
            "http://[::1]:8090/ready"
        );
        assert_eq!(
            target("localhost:8091").expect("DNS bind").as_str(),
            "http://localhost:8091/ready"
        );
        for bad in [
            "user:secret@localhost:8090",
            "localhost:8090/live?",
            "localhost:8090/#fragment",
            "127.0.0.1:0",
        ] {
            assert!(target(bad).is_err(), "{bad}");
        }
    }
    #[tokio::test]
    async fn only_ready_200_succeeds_without_business_credentials() {
        for status in [200, 204, 401, 403, 500, 503] {
            let listener = TcpListener::bind("127.0.0.1:0")
                .await
                .expect("owned listener");
            let bind = listener.local_addr().expect("address").to_string();
            let server = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.expect("probe");
                let mut raw = [0; 4096];
                let count = stream.read(&mut raw).await.expect("headers");
                let request = String::from_utf8_lossy(&raw[..count]);
                assert!(request.starts_with("GET /ready HTTP/1.1\r\n"));
                assert!(!request.to_ascii_lowercase().contains("authorization:"));
                stream.write_all(format!("HTTP/1.1 {status} Probe\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes()).await.expect("reply");
            });
            assert_eq!(check(&bind).await.is_ok(), status == 200);
            server.await.expect("probe peer");
        }
    }
    #[tokio::test]
    async fn redirects_cannot_turn_an_unready_service_into_success() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("owned listener");
        let bind = listener.local_addr().expect("address").to_string();
        let destination = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("owned redirect destination");
        let next = destination.local_addr().expect("destination");
        async fn serve(listener: TcpListener, response: String) {
            let mut peers = tokio::task::JoinSet::new();
            loop {
                let (mut stream, _) = listener.accept().await.expect("owned peer");
                let response = response.clone();
                peers.spawn(async move {
                    let mut raw = [0; 4096];
                    if let Ok(Ok(count)) =
                        tokio::time::timeout(Duration::from_millis(500), stream.read(&mut raw))
                            .await
                    {
                        if count > 0 {
                            let _ = stream.write_all(response.as_bytes()).await;
                        }
                    }
                });
                while peers.try_join_next().is_some() {}
            }
        }
        let server = tokio::spawn(serve(listener, format!("HTTP/1.1 302 Found\r\nLocation: http://{next}/ready\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")));
        let destination = tokio::spawn(serve(
            destination,
            "HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".into(),
        ));
        // Prove that following this exact redirect really would reach a healthy
        // endpoint, then require the actual probe's specific non-200 refusal.
        // A passive accept timeout could confuse an accept error or unrelated
        // local connection with a redirect made by the readiness client.
        let following = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::limited(1))
            .timeout(Duration::from_millis(1500))
            .build()
            .expect("control client")
            .get(format!("http://{bind}/ready"))
            .send()
            .await;
        let result = check(&bind).await;
        server.abort();
        destination.abort();
        assert_eq!(
            following.expect("healthy redirect control").status(),
            reqwest::StatusCode::OK
        );
        assert_eq!(
            result
                .expect_err("readiness must not follow a healthy redirect")
                .to_string(),
            "Freight storage is not ready"
        );
    }
    #[tokio::test]
    async fn stalled_response_expires_instead_of_hanging_a_health_check() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("owned listener");
        let bind = listener.local_addr().expect("address").to_string();
        let server = tokio::spawn(async move {
            let (_stream, _) = listener.accept().await.expect("probe");
            tokio::time::sleep(Duration::from_secs(10)).await;
        });
        let error = tokio::time::timeout(Duration::from_secs(5), check(&bind))
            .await
            .expect("bounded probe")
            .expect_err("stalled service");
        assert!(error
            .downcast_ref::<reqwest::Error>()
            .is_some_and(|e| e.is_timeout()));
        server.abort();
    }
}
