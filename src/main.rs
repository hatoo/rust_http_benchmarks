use std::sync::atomic::{AtomicUsize, Ordering};
use warp::Filter;

async fn run<T>(name: &str, f: impl std::future::Future<Output = T>) -> T {
    let now = std::time::Instant::now();
    let r = f.await;
    println!("{}: {:?}", name, now.elapsed());
    r
}

async fn create_hyper_conn() -> anyhow::Result<hyper::client::conn::SendRequest<hyper::Body>> {
    let stream = tokio::net::TcpStream::connect("127.0.0.1:8080").await?;
    // stream.set_nodelay(true)?;
    // stream.set_keepalive(std::time::Duration::from_secs(1).into())?;
    let (send, conn) = hyper::client::conn::handshake(stream).await?;
    tokio::spawn(conn);
    Ok(send)
}

async fn hyper_do_req(counter: &AtomicUsize) -> anyhow::Result<usize> {
    use hyper::Request;
    use tokio::stream::StreamExt;

    let mut send_request = create_hyper_conn().await?;
    let mut len = 0;

    while counter.fetch_add(1, Ordering::Relaxed) < 1000 {
        while futures::future::poll_fn(|ctx| send_request.poll_ready(ctx))
            .await
            .is_err()
        {
            send_request = create_hyper_conn().await?;
        }
        let request = Request::builder()
            .method("GET")
            .uri("http://127.0.0.1:8080/")
            .body(hyper::Body::empty())?;
        let res = send_request.send_request(request).await?;
        let (_parts, mut stream) = res.into_parts();
        while let Some(chunk) = stream.next().await {
            len += chunk?.len();
        }
    }
    Ok(len)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tokio::spawn(async {
        let hello = warp::any().map(|| "Hello, World!");
        warp::serve(hello).run(([0, 0, 0, 0], 8080)).await
    });

    run("reqwest naive serial 1000", async {
        for _ in 0..1000 {
            reqwest::get("http://127.0.0.1:8080").await.unwrap();
        }
    })
    .await;

    run("reqwest serial 1000", async {
        let client = reqwest::Client::new();
        for _ in 0..1000 {
            client.get("http://127.0.0.1:8080").send().await.unwrap();
        }
    })
    .await;

    run("reqwest naive para 1000", async {
        let client = reqwest::Client::new();
        let futures = (0..1000)
            .map(|_| {
                let client = client.clone();
                tokio::spawn(async move { client.get("http://127.0.0.1:8080").send().await })
            })
            .collect::<Vec<_>>();
        for f in futures {
            let _ = f.await;
        }
    })
    .await;

    run("reqwest naive para join_all 1000", async {
        let client = reqwest::Client::new();
        let futures = futures::future::join_all((0..1000).map(|_| {
            let client = client.clone();
            tokio::spawn(async move { client.get("http://127.0.0.1:8080").send().await })
        }));
        futures.await;
    })
    .await;

    run("reqwest para workers 100 1000", async {
        let counter = AtomicUsize::new(0);
        let client = reqwest::Client::new();
        let _ = futures::future::join_all((0..100).map(|_| async {
            let client = client.clone();
            while counter.fetch_add(1, Ordering::Relaxed) < 1000 {
                let _ = client.get("http://127.0.0.1:8080").send().await;
            }
        }))
        .await;
    })
    .await;

    assert_eq!(
        run("hyper para wokers 100 1000", async {
            let counter = AtomicUsize::new(0);
            futures::future::join_all(
                (0..100).map(|_| async { hyper_do_req(&counter).await.unwrap() }),
            )
            .await
        })
        .await
        .into_iter()
        .sum::<usize>(),
        13000
    );

    Ok(())
}
