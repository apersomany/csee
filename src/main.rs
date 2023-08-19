use anyhow::{anyhow, Ok, Result};
use nix::sched::{setns, CloneFlags};
use std::{
    future::Future,
    os::fd::AsRawFd,
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
    task::JoinHandle,
    time::sleep,
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
    let _ = exec("ip netns delete server").await;
    let _ = exec("ip netns delete client").await;

    spawn_task(async {
        ctrl_c().await?;
        exit().await
    });

    exec("ip netns add server").await?;
    exec("ip netns add client").await?;

    let server = File::open("/var/run/netns/server").await?;
    let client = File::open("/var/run/netns/client").await?;

    exec("ip link add dev server netns server type veth peer name client netns client").await?;
    exec("ip netns exec server ip link set dev server up").await?;
    exec("ip netns exec client ip link set dev client up").await?;
    exec("ip netns exec server ip addr add dev server 10.1.1.1/24").await?;
    exec("ip netns exec client ip addr add dev client 10.1.1.2/24").await?;

    exec("ip netns exec server tc qdisc add dev server root netem delay 10ms").await?;
    exec("ip netns exec client tc qdisc add dev client root netem delay 10ms").await?;
    // exec("ip netns exec client tc qdisc add dev client root netem loss 10%@").await?;

    setns(server.as_raw_fd(), CloneFlags::empty())?;
    let lstn = TcpListener::bind("10.1.1.1:1234").await?;
    spawn_task(async move {
        let mut strm = lstn.accept().await?.0;
        loop {
            let data = strm.read_u128().await?;
            strm.write_u128(data).await?;
        }
    });

    setns(client.as_raw_fd(), CloneFlags::empty())?;
    let sock = TcpSocket::new_v4()?;
    sock.bind("10.1.1.2:1234".parse()?)?;
    let mut strm = sock.connect("10.1.1.1:1234".parse()?).await?;
    spawn_task(async move {
        loop {
            let inst = Instant::now();
            strm.write_u128(inst.elapsed().as_millis()).await?;
            let data = strm.read_u128().await?;
            println!("{}", inst.elapsed().as_millis() - data);
        }
    });

    sleep(Duration::MAX).await;

    exit().await
}
