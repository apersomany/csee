use anyhow::Result;
use nix::sched::{setns, CloneFlags};
use std::{
    fs::File,
    io::{ErrorKind, Read, Write},
    net::{TcpListener, TcpStream},
    thread::spawn,
    time::{Duration, Instant},
};

use crate::{exec, tcp_info, Measurement};

pub fn init_old() -> Result<()> {
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
    setns(File::open("/var/run/netns/server")?, CloneFlags::empty())?;
    let server = TcpListener::bind("10.1.1.1:1234")?;
    spawn(move || {
        while let Ok((mut stream, _)) = server.accept() {
            while let Ok(_) = stream.read_exact(&mut [0; 1460]) {}
        }
    });
    setns(File::open("/var/run/netns/client")?, CloneFlags::empty())?;
    exec("nft add table ip filter", Some("client"))?;
    Ok(())
}

pub fn simulate_old(r: f64, p: f64) -> Result<Vec<Measurement>> {
    exec(
        format!("tc qdisc add dev client root netem delay {r}s loss {p}"),
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
        format!("tc qdisc del dev client root netem delay {r}s loss {p}"),
        Some("client"),
    )?;
    Ok(measurements)
}
