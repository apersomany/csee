use std::{
    collections::HashMap,
    fs::File,
    io::{Read, Write},
    mem::{size_of, MaybeUninit},
    net::{TcpListener, TcpStream},
    ops::AddAssign,
    os::fd::AsRawFd,
    process::Command,
    thread::spawn,
    time::{Duration, Instant},
};

use anyhow::{Error, Result};
use nix::{
    libc::{getsockopt, SOL_TCP, TCP_INFO},
    sched::{setns, CloneFlags},
};
// use plotters::prelude::*;

#[derive(Debug)]
#[repr(C)]
struct TcpInfo {
    tcpi_state: u8,
    tcpi_ca_state: u8,
    tcpi_retransmits: u8,
    tcpi_probes: u8,
    tcpi_backoff: u8,
    tcpi_options: u8,
    tcpi_snd_wscale: u8,
    tcpi_rcv_wscale: u8,
    tcpi_rto: u32,
    tcpi_ato: u32,
    tcpi_snd_mss: u32,
    tcpi_rcv_mss: u32,
    tcpi_unacked: u32,
    tcpi_sacked: u32,
    tcpi_lost: u32,
    tcpi_retrans: u32,
    tcpi_fackets: u32,
    tcpi_last_data_sent: u32,
    tcpi_last_ack_sent: u32,
    tcpi_last_data_recv: u32,
    tcpi_last_ack_recv: u32,
    tcpi_pmtu: u32,
    tcpi_rcv_ssthresh: u32,
    tcpi_rtt: u32,
    tcpi_rttvar: u32,
    tcpi_snd_ssthresh: u32,
    tcpi_snd_cwnd: u32,
    tcpi_advmss: u32,
    tcpi_reordering: u32,
    tcpi_rcv_rtt: u32,
    tcpi_rcv_space: u32,
    tcpi_total_retrans: u32,
}

impl TcpInfo {
    fn read(stream: &TcpStream) -> Result<Self> {
        unsafe {
            let mut tcp_info = MaybeUninit::<Self>::uninit();
            let mut sock_len = size_of::<TcpInfo>() as u32;
            let ret = getsockopt(
                stream.as_raw_fd(),
                SOL_TCP,
                TCP_INFO,
                tcp_info.as_mut_ptr().cast(),
                &mut sock_len,
            );
            if ret != 0 {
                Err(nix::Error::last().into())
            } else {
                Ok(tcp_info.assume_init())
            }
        }
    }
}

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
            while let Ok(_) = stream.write_all([0; 1448].as_ref()) {}
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
    let rule = format!("delay {}ms loss {}%", r * 1000.0, p * 100.0);
    exec(
        format!("tc qdisc add dev server root netem {rule}"),
        Some("server"),
    )?;
    let mut stream = TcpStream::connect("10.1.1.1:1234")?;
    let mut buffer = [0; 1448];
    let mut segcnt = 0;
    // let mut points = Vec::new();
    let now = Instant::now();
    loop {
        let tcp_info = TcpInfo::read(&stream)?;
        println!("{:#?}", tcp_info.tcpi_snd_cwnd);
        stream.read_exact(&mut buffer)?;
        segcnt += 1;
        if now.elapsed() > Duration::from_secs(60) {
            break;
        }
    }
    exec(
        format!("tc qdisc del dev server root netem {rule}"),
        Some("server"),
    )?;
    Ok(segcnt as f64 / now.elapsed().as_secs_f64())
}

fn main() {
    init().expect("failed to initialize");
    for i in 2..10 {
        println!("[est] {:5.0}", estimate(0.1, 0.00001 * i as f64));
        println!("[sim] {:5.0}", simulate(0.1, 0.00001 * i as f64).unwrap());
    }
}
