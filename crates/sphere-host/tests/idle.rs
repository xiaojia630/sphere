//! A host with nothing to do must not wake up: it runs all day, so every
//! wake-up it makes while idle costs battery.
//!
//! Counts how often the runtime's threads resume work while followers watch a
//! silent child. This is the only test in its file on purpose: another test's
//! children would send this process SIGCHLD and show up as wake-ups.

use sphere_host::{serve, FollowItem, HostClient, ServeOptions, SpawnParams, Stream};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

async fn connect(socket: &Path) -> HostClient {
    for _ in 0..100 {
        if let Ok(c) = HostClient::connect(socket).await {
            if c.ping().await.is_ok() {
                return c;
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("host did not start");
}

#[test]
fn followers_of_a_silent_child_do_not_wake_the_host_but_still_hear_its_exit() {
    let wakes = Arc::new(AtomicU64::new(0));
    let counter = wakes.clone();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .on_thread_unpark(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        })
        .build()
        .unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let socket = tmp.path().join("run/host.sock");
    let opts = ServeOptions {
        socket: socket.clone(),
        dir: tmp.path().join("out"),
        reap_orphans: false,
    };
    rt.spawn(async move {
        let _ = serve(opts).await;
    });

    let (client, child, mut followers) = rt.block_on(async {
        let c = connect(&socket).await;
        let child = c
            .spawn(SpawnParams {
                cmd: "/bin/sh".into(),
                args: vec!["-c".into(), "sleep 300".into()],
                ..Default::default()
            })
            .await
            .unwrap();
        let mut followers = vec![];
        for stream in [Stream::Out, Stream::Err] {
            followers.push(c.follow(&child.id, stream, 0).await.unwrap());
        }
        (c, child, followers)
    });

    // Let the host settle after the last request, then count two seconds of
    // nothing. A timer ticking even every half second wakes it four times or
    // more; the allowance is for a stray wake-up on a busy machine.
    std::thread::sleep(Duration::from_millis(300));
    let before = wakes.load(Ordering::SeqCst);
    std::thread::sleep(Duration::from_secs(2));
    let idle = wakes.load(Ordering::SeqCst) - before;
    assert!(idle <= 2, "an idle host woke {idle} times in two seconds");

    // Nothing ticks, yet the exit still reaches both followers.
    rt.block_on(async {
        client
            .signal(&child.id, serde_json::json!("KILL"), true)
            .await
            .unwrap();
        for f in &mut followers {
            let item = tokio::time::timeout(Duration::from_secs(10), f.next())
                .await
                .expect("the follower never heard of the exit")
                .expect("the host closed the connection instead of reporting the exit")
                .unwrap();
            match item {
                FollowItem::Exit(status) => assert_eq!(status.signal, Some(libc::SIGKILL)),
                other => panic!("expected the exit, got {other:?}"),
            }
        }
    });
}
