use std::{
    fs::File,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    ops::AddAssign,
    process::Command,
    thread::spawn,
    time::{Duration, Instant},
};

use anyhow::{Error, Result};
use nix::sched::{setns, CloneFlags};
// use plotters::prelude::*;

fn exec(command: impl AsRef<str>, netns: Option<&str>) -> Result<String> {
    let mut command = command.as_ref().split_whitespace();
    let output = if let Some(netns) = netns {
        Command::new("ip")
            .args(["netns", "exec", netns])
            .args(command)
            .output()?
    } else {
        Command::new(command.next().unwrap())
            .args(command)
            .output()?
    };
    if output.status.success() {
        Ok(String::from_utf8(output.stdout)?)
    } else {
        Err(Error::msg(String::from_utf8(output.stderr)?))
    }
}

fn init() -> Result<()> {
    let _ = exec(format!("ip netns delete server"), None);
    let _ = exec(format!("ip netns delete client"), None);
    exec(format!("ip netns add server"), None)?;
    exec(format!("ip netns add client"), None)?;
    exec(
        "ip link add dev server netns server type veth peer name client netns client",
        None,
    )?;
    exec(format!("ip link set dev server up"), Some("server"))?;
    exec(format!("ip link set dev client up"), Some("client"))?;
    exec(
        format!("ip addr add dev server 10.1.1.1/24"),
        Some("server"),
    )?;
    exec(
        format!("ip addr add dev client 10.1.1.2/24"),
        Some("client"),
    )?;
    setns(File::open("/var/run/netns/server")?, CloneFlags::empty())?;
    let server = TcpListener::bind("10.1.1.1:1234")?;
    spawn(move || {
        while let Ok((mut stream, _)) = server.accept() {
            while let Ok(_) = stream.write_all([0; 1024].as_ref()) {}
        }
    });
    setns(File::open("/var/run/netns/client")?, CloneFlags::empty())?;
    Ok(())
}

fn estimate(d: f64, p: f64) -> f64 {
    const C: f64 = 0.04;
    f64::max(
        (C * 3.7 / 1.2).powf(0.25) * (d / p).powf(0.75) / d,
        (3.0 / 2.0 / p).powf(0.5) / d,
    )
}

fn simulate(r: f64, p: f64) -> Result<f64> {
    let rule = format!("latency {}ms loss {}%", r * 1000.0, p * 100.0);
    exec(
        format!("tc qdisc add dev server root netem {rule}"),
        Some("server"),
    )?;
    let mut stream = TcpStream::connect("10.1.1.1:1234")?;
    let mut buffer = [0; 1024];
    let mut length = 0;
    let now = Instant::now();
    loop {
        stream.read_exact(&mut buffer)?;
        length.add_assign(1);
        if now.elapsed() > Duration::from_secs(60) {
            break;
        }
    }
    exec(
        format!("tc qdisc del dev server root netem {rule}"),
        Some("server"),
    )?;
    Ok(length as f64 / 60.0)
}

fn main() {
    init().expect("failed to initialize");
    for i in 1..10 {
        println!("[est] {:5.0}", estimate(0.1, 0.00001 * i as f64));
        println!("[sim] {:5.0}", simulate(0.1, 0.00001 * i as f64).unwrap());
    }
}
