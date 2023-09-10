use anyhow::{anyhow, Ok, Result};
use nix::sched::{setns, CloneFlags};
use std::{
    future::Future,
    process,
    time::{Duration, Instant},
};
use tokio::{
    fs::File,
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpSocket},
    process::Command,
    signal::ctrl_c,
    spawn,
    task::{JoinHandle, JoinSet},
};

fn spawn_task(future: impl Future<Output = Result<()>> + Send + 'static) -> JoinHandle<()> {
    spawn(async move {
        if let Err(e) = future.await {
            println!("{}", e)
        }
    })
}

async fn exec(command: impl AsRef<str>) -> Result<()> {
    let mut command = command.as_ref().split_whitespace();
    let output = Command::new(command.next().unwrap())
        .args(command)
        .output()
        .await?;
    if output.status.success() {
        Ok(())
    } else {
        Err(anyhow!("{}", String::from_utf8(output.stderr)?))
    }
}

async fn exit() -> Result<()> {
    exec("ip netns delete server").await?;
    exec("ip netns delete client").await?;
    process::exit(0);
}

#[tokio::main]
async fn main() -> Result<()> {
    // try to delete network namespace
    let _ = exec("ip netns delete server").await;
    let _ = exec("ip netns delete client").await;

    // handle sigint
    spawn_task(async {
        ctrl_c().await?;
        exit().await
    });

    // create network namespace
    exec("ip netns add server").await?;
    exec("ip netns add client").await?;

    // interface setup
    exec("ip link add dev server netns server type veth peer name client netns client").await?;
    exec("ip netns exec server ip link set dev server up").await?;
    exec("ip netns exec client ip link set dev client up").await?;
    exec("ip netns exec server ip addr add dev server 10.1.1.1/24").await?;
    exec("ip netns exec client ip addr add dev client 10.1.1.2/24").await?;

    // run the measurements
    for (impairment, quantities) in [
        (
            "delay",
            (0..5).map(|e| format!("{}ms", e * 20)).collect::<Vec<_>>(),
        ),
        (
            "loss",
            (0..5).map(|e| format!("{}%", e)).collect::<Vec<_>>(),
        ),
    ] {
        for quantity in quantities {
            println!("Measuring {} {}", impairment, quantity);

            // create conditions
            exec(format!(
                "ip netns exec client tc qdisc add dev client root netem {} {}",
                impairment, quantity
            ))
            .await?;
            exec(format!(
                "ip netns exec server tc qdisc add dev server root netem {} {}",
                impairment, quantity
            ))
            .await?;

            // create server socket
            let server = File::open("/var/run/netns/server").await?;
            setns(server, CloneFlags::empty())?;
            let server = TcpListener::bind("10.1.1.1:1234").await?;

            // create client socket
            let client = File::open("/var/run/netns/client").await?;
            setns(client, CloneFlags::empty())?;
            let client = TcpSocket::new_v4()?;
            client.bind("10.1.1.2:0".parse()?)?;

            let mut join_set = JoinSet::new();

            // spawn server
            join_set.spawn(spawn_task(async move {
                let mut stream = server.accept().await?.0;
                let mut buffer = [0; 1024];
                let mut amount = 0;
                let now = Instant::now();
                while now.elapsed() < Duration::from_secs(1) {
                    amount = amount + stream.read(&mut buffer).await?;
                }
                println!("{}Mb/s", amount * 8 / 1024 / 1024 / 1);
                Ok(())
            }));

            // spawn client
            join_set.spawn(spawn_task(async {
                let mut stream = client.connect("10.1.1.1:1234".parse()?).await?;
                let now = Instant::now();
                while now.elapsed() < Duration::from_secs(1) {
                    stream.write(&[0; 1024]).await?;
                }
                Ok(())
            }));

            // await both futures
            while join_set.join_next().await.is_some() {}

            // delete conditions
            exec(format!(
                "ip netns exec client tc qdisc del dev client root netem {} {}",
                impairment, quantity
            ))
            .await?;
            exec(format!(
                "ip netns exec server tc qdisc del dev server root netem {} {}",
                impairment, quantity
            ))
            .await?;
        }
    }

    exit().await
}
