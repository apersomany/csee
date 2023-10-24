use std::{
    fs::File,
    io::{ErrorKind, Read, Write},
    net::{TcpListener, TcpStream},
    thread::spawn,
    time::{Duration, Instant},
};

use anyhow::Result;
use nix::sched::{setns, CloneFlags};

use crate::{exec, tcp_info, Measurement};

pub fn init_new() -> Result<()> {
    let _ = exec("ip netns delete server", None);
    let _ = exec("ip netns delete client", None);
    exec("ip netns add server", None)?;
    exec("ip netns add client", None)?;
    exec(
        "ip link add dev server netns server type veth peer name client netns client",
        None,
    )?;
    exec("ip addr add dev server 10.1.1.1/24", Some("server"))?;
    exec("ip addr add dev client 10.1.1.2/24", Some("client"))?;
    exec("ip link set dev server up mtu 1500", Some("server"))?;
    exec("ip link set dev client up mtu 1500", Some("client"))?;
    exec("ethtool -K server tso off", Some("server"))?;
    exec("ethtool -K client tso off", Some("client"))?;
    setns(File::open("/var/run/netns/server")?, CloneFlags::empty())?;
    let server = TcpListener::bind("10.1.1.1:1234")?;
    spawn(move || {
        while let Ok((mut stream, _)) = server.accept() {
            while let Ok(_) = stream.read_exact(&mut [0; 1460]) {}
        }
    });
    setns(File::open("/var/run/netns/client")?, CloneFlags::empty())?;
    Ok(())
}

pub fn simulate_new(r: f64, p: f64) -> Result<Vec<Measurement>> {
    exec("nft add table ip filter", Some("server"))?;
    exec(
        "nft add chain ip filter input { type filter hook input priority 0; }",
        Some("server"),
    )?;
    exec("nft add rule filter input counter", Some("server"))?;
    exec(
        "nft add rule filter input meta length > 1500 counter drop",
        Some("server"),
    )?;
    exec(
        format!(
            "nft add rule filter input numgen inc mod {} == {} counter drop",
            p.recip().round(),
            p.recip().round() - 1.0,
        ),
        Some("server"),
    )?;
    exec(
        format!("tc qdisc add dev client root netem delay {r}s"),
        Some("client"),
    )?;
    let mut measurements = Vec::new();
    let mut stream = TcpStream::connect("10.1.1.1:1234")?;
    stream.set_nonblocking(true)?;
    let mut segcnt = 0;
    let now = Instant::now();
    while now.elapsed() < Duration::from_secs(60) {
        let now = Instant::now();
        if let Err(e) = stream.write_all(&[0; 1460]) {
            if e.kind() == ErrorKind::WouldBlock {
                while now.elapsed() < Duration::from_millis(100) {}
                measurements.push(Measurement {
                    bytes_transferred: segcnt * 1460,
                    congestion_window: tcp_info(&stream)?.tcpi_snd_cwnd as usize,
                });
            } else {
                Err(e)?;
            }
        } else {
            segcnt = segcnt + 1;
        }
    }
    exec(
        format!("tc qdisc del dev client root netem delay {r}s"),
        Some("client"),
    )?;
    exec("nft flush ruleset", Some("server"))?;
    Ok(measurements)
}
