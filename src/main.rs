mod tcp_info;
use anyhow::{Error, Result};
use nix::sched::{setns, CloneFlags};
use plotters::prelude::*;
use std::{
    fs::File,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    process::Command,
    thread::spawn,
    time::{Duration, Instant},
};
use tcp_info::TcpInfo;

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
    // try to clean up from possible last runs
    let _ = exec("ip netns delete server", None);
    let _ = exec("ip netns delete client", None);
    // create the namespaces
    exec("ip netns add server", None)?;
    exec("ip netns add client", None)?;
    // create the devices
    exec(
        "ip link add dev server netns server type veth peer name client netns client",
        None,
    )?;
    // add addresses to the devices
    exec("ip addr add dev server 10.1.1.1/24", Some("server"))?;
    exec("ip addr add dev client 10.1.1.2/24", Some("client"))?;
    // bring up the devices and set the mtu
    exec("ip link set dev server mtu 1500 up", Some("server"))?;
    exec("ip link set dev client mtu 1500 up", Some("client"))?;
    // idk why the hell tso is enabled for veth but disable it anyways
    exec("ethtool -K server tso off", Some("server"))?;
    exec("ethtool -K client tso off", Some("client"))?;
    setns(File::open("/var/run/netns/server")?, CloneFlags::empty())?;
    // create the server listener (although it is actually the receiver of data)
    let server = TcpListener::bind("10.1.1.1:1234")?;
    spawn(move || {
        while let Ok((mut stream, _)) = server.accept() {
            while let Ok(_) = stream.read(&mut [0; 1460]) {}
        }
    });
    setns(File::open("/var/run/netns/client")?, CloneFlags::empty())?;
    exec("nft add table ip filter", Some("client"))?;
    Ok(())
}

fn estimate(r: f64, p: f64) -> Result<f64> {
    const C: f64 = 41.0 / 1024.0;
    Ok(f64::max(
        (C * 3.7 / 1.2).powf(0.25) * (r / p).powf(0.75) / r,
        (3.0 / 2.0 / p).powf(0.5) / r,
    ))
}

fn simulate(r: f64, p: f64) -> Result<f64> {
    exec("nft add table ip filter", Some("server"))?;
    exec(
        "nft add chain ip filter input { type filter hook input priority 0; }",
        Some("server"),
    )?;
    exec("nft add rule filter input counter", Some("server"))?;
    exec(
        "nft add rule filter input meta length > 1500 counter reject",
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
        format!("tc qdisc add dev client root netem delay {}ms", r * 1000.0),
        Some("client"),
    )?;
    let mut points = Vec::new();
    let mut stream = TcpStream::connect("10.1.1.1:1234")?;
    let mut segcnt = 0;
    let now = Instant::now();
    'main: loop {
        let tcp_info = TcpInfo::read(&stream)?;
        for _ in 0..tcp_info.tcpi_snd_cwnd {
            stream.write_all(&[0; 1460])?;
            if now.elapsed() > Duration::from_secs(30) {
                segcnt = segcnt + 1;
            }
            if now.elapsed() > Duration::from_secs(60) {
                break 'main;
            }
        }
        points.push((now.elapsed().as_secs_f64(), tcp_info.tcpi_snd_cwnd))
    }
    let throughput = segcnt as f64 / 30.0;
    exec(
        format!("tc qdisc del dev client root netem delay {}ms", r * 1000.0),
        Some("client"),
    )?;
    println!("[nft] {}", exec("nft list ruleset", Some("server"))?);
    exec("nft flush ruleset", Some("server"))?;
    let path = format!("out/{r}_{p}.png");
    let root = BitMapBackend::new(&path, (2048, 1024)).into_drawing_area();
    root.fill(&WHITE)?;
    let mut chart = ChartBuilder::on(&root)
        .caption(format!("CWND (Packets) vs Time (s)"), 32)
        .margin_right(64)
        .margin_top(64)
        .x_label_area_size(64)
        .y_label_area_size(64)
        .build_cartesian_2d(r..now.elapsed().as_secs_f64(), 0..8192u32)?;
    chart
        .configure_mesh()
        .label_style(("sans-serif", 24))
        .draw()?;
    chart.draw_series(LineSeries::new(points.into_iter(), &BLACK))?;
    root.present()?;
    Ok(throughput)
}

fn main() {
    init().expect("failed to initialize");
    let path = format!("out/main.png");
    let root = BitMapBackend::new(&path, (2048, 1024)).into_drawing_area();
    root.fill(&WHITE).unwrap();
    let mut chart = ChartBuilder::on(&root)
        .caption(format!("Average CWND (Packets) vs P (Probability)"), 32)
        .margin_right(64)
        .x_label_area_size(64)
        .y_label_area_size(64)
        .build_cartesian_2d(0.00001..0.00010f64, 0.0..8192.0f64)
        .unwrap();
    chart
        .configure_mesh()
        .label_style(("sans-serif", 24))
        .draw()
        .unwrap();
    chart
        .draw_series(LineSeries::new(
            (1..10).map(|i| {
                (
                    0.00001 * i as f64,
                    simulate(0.1, 0.00001 * i as f64).unwrap(),
                )
            }),
            &BLACK,
        ))
        .unwrap();
}
