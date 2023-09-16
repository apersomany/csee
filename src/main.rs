use std::{
    fs::File,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    os::fd::AsRawFd,
    process::Command,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    thread::spawn,
    time::Duration,
};

use anyhow::{Error, Result};
use plotters::prelude::*;

fn execute(command: impl AsRef<str>, netns: Option<&str>) -> Result<()> {
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
        Ok(())
    } else {
        Err(Error::msg(String::from_utf8(output.stderr)?))
    }
}

fn set_netns(netns: &str) -> Result<()> {
    unsafe {
        if libc::setns(
            File::open(format!("/var/run/netns/{netns}"))?.as_raw_fd(),
            0,
        ) < 0
        {
            Err(std::io::Error::last_os_error().into())
        } else {
            Ok(())
        }
    }
}

fn init() -> Result<()> {
    // create network namespaces
    for side in ["server", "client"] {
        execute(format!("ip netns add {side}"), None)?;
    }
    // create network interfaces
    execute(
        "ip link add dev server netns server type veth peer name client netns client",
        None,
    )?;
    // configure the network interfaces
    for (side, addr) in [("server", "10.1.1.1/24"), ("client", "10.1.1.2/24")] {
        execute(format!("ip link set dev {side} up"), Some(side))?;
        execute(format!("ip addr add dev {side} {addr}"), Some(side))?;
    }
    set_netns("server")?;
    let server = TcpListener::bind("10.1.1.1:1234")?;
    spawn(move || {
        while let Ok((mut stream, _)) = server.accept() {
            while let Ok(_) = stream.write_all([0; 1024].as_ref()) {}
        }
    });
    set_netns("client")?;
    Ok(())
}

fn clean() -> Result<()> {
    // delete network namespaces
    for side in ["server", "client"] {
        execute(format!("ip netns delete {side}"), None)?;
    }
    Ok(())
}

fn run_trial(impairment: &str, quantity: &str) -> Result<()> {
    const DURATION: f64 = 0.05f64;
    const DIVISION: usize = 200;
    execute(
        format!("tc qdisc add dev server root netem {impairment} {quantity} rate 1gbit"),
        Some("server"),
    )?;
    let mut stream = TcpStream::connect("10.1.1.1:1234")?;
    let mut buffer = Box::new([0; 1024]);
    let amount = Arc::new(AtomicUsize::new(0));
    let speeds = {
        let amount = amount.clone();
        spawn(move || {
            let mut speeds = Vec::new();
            for _ in 0..DIVISION {
                std::thread::sleep(Duration::from_secs_f64(DURATION));
                speeds.push(amount.swap(0, Ordering::AcqRel) as f64 / DURATION / 2f64.powf(17f64));
            }
            speeds
        })
    };
    while !speeds.is_finished() {
        amount.fetch_add(stream.read(buffer.as_mut())?, Ordering::AcqRel);
    }
    execute(
        format!("tc qdisc del dev server root netem {impairment} {quantity} rate 1gbit"),
        Some("server"),
    )?;
    let path = format!("{impairment}_{quantity}.png");
    let root = BitMapBackend::new(&path, (1024, 512)).into_drawing_area();
    root.fill(&WHITE)?;
    let mut chart =
        ChartBuilder::on(&root).build_cartesian_2d(0..DIVISION, 0f64..2f64.powf(10f64))?;
    chart.draw_series(LineSeries::new(
        speeds.join().unwrap().into_iter().enumerate(),
        &RED,
    ))?;
    root.present()?;
    Ok(())
}

fn main() {
    let _ = clean();
    init().unwrap();
    for i in 0..6 {
        run_trial("duplicate", &format!("{}%", i * 10)).unwrap();
    }
    clean().unwrap();
}
