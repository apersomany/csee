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
    time::Instant,
};

use anyhow::{Error, Result};
use plotters::prelude::*;

fn execute(command: impl AsRef<str>, netns: Option<&str>) -> Result<String> {
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
    for side in ["server", "client"] {
        execute(format!("ip netns add {side}"), None)?;
    }
    execute(
        "ip link add dev server netns server type veth peer name client netns client",
        None,
    )?;
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
    for side in ["server", "client"] {
        execute(format!("ip netns delete {side}"), None)?;
    }
    Ok(())
}

const CNT: usize = 200;
const LEN: f64 = 0.050;
const TOT: f64 = CNT as f64 * LEN;

fn simulate(
    impairment: &str,
    quantities: impl Iterator<Item = (String, Option<f64>)>,
) -> Result<()> {
    let quantities = quantities.collect::<Vec<_>>();
    let data = quantities
        .iter()
        .map(|(quantity, expected)| {
            println!("Simulating {impairment} {quantity}");
            execute(
                format!(
                    "tc qdisc add dev server root netem {impairment} {quantity} rate 1073741824bit"
                ),
                Some("server"),
            )?;
            let mut stream = TcpStream::connect("10.1.1.1:1234")?;
            let mut buffer = Box::new([0; 1024]);
            let amount = Arc::new(AtomicUsize::new(0));
            let thread = {
                let amount = amount.clone();
                spawn(move || {
                    let mut data = Vec::new();
                    for _ in 0..CNT {
                        let now = Instant::now();
                        while now.elapsed().as_secs_f64() < LEN {}
                        data.push(
                            amount.swap(0, Ordering::AcqRel) as f64 / LEN / 2f64.powf(20f64 - 3f64),
                        );
                    }
                    data
                })
            };
            while !thread.is_finished() {
                amount.fetch_add(stream.read(buffer.as_mut())?, Ordering::AcqRel);
            }
            let data = thread.join().unwrap();
            execute(
                format!(
                    "tc qdisc del dev server root netem {impairment} {quantity} rate 1073741824bit"
                ),
                Some("server"),
            )?;
            let path = format!("out/{}_{quantity}.png", impairment.replace(" ", "_"));
            let root = BitMapBackend::new(&path, (1024, 512)).into_drawing_area();
            root.fill(&WHITE)?;
            let mut chart = ChartBuilder::on(&root)
                .caption(
                    format!("{impairment} ({quantity})"),
                    (FontFamily::SansSerif, 20).into_font(),
                )
                .margin(10)
                .set_label_area_size(LabelAreaPosition::Bottom, 40)
                .set_label_area_size(LabelAreaPosition::Left, 80)
                .build_cartesian_2d(0f64..(TOT - LEN), 0.0..2f64.powf(10f64))?;
            chart
                .configure_mesh()
                .x_desc("time")
                .y_desc("speed")
                .x_label_formatter(&|x| format!("{:2.1}s", x))
                .y_label_formatter(&|y| format!("{y}mb/s"))
                .draw()?;
            chart
                .draw_series(LineSeries::new(
                    data.clone()
                        .into_iter()
                        .enumerate()
                        .map(|(i, e)| (i as f64 * LEN, e)),
                    &RED,
                ))?
                .label("measured")
                .legend(|(x, y)| Rectangle::new([(x, y - 5), (x + 10, y + 5)], RED.filled()));
            if let Some(expected) = expected {
                chart
                    .draw_series(LineSeries::new(
                        [(0f64, *expected), (TOT, *expected)],
                        &BLUE,
                    ))?
                    .label("expected")
                    .legend(|(x, y)| Rectangle::new([(x, y - 5), (x + 10, y + 5)], BLUE.filled()));
            }
            chart
                .configure_series_labels()
                .border_style(&BLACK)
                .draw()?;
            root.present()?;
            println!(
                "{}mb/s",
                data.clone().into_iter().fold(0.0, |a, b| a + b) / CNT as f64
            );
            Ok(data.into_iter().fold(0.0, |a, b| a + b) / CNT as f64)
        })
        .collect::<Result<Vec<_>>>()?;
    let path = format!("out/{}.png", impairment.replace(" ", "_"));
    let root = BitMapBackend::new(&path, (1024, 512)).into_drawing_area();
    root.fill(&WHITE)?;
    let mut chart = ChartBuilder::on(&root)
        .caption(
            format!("{impairment} vs speed"),
            (FontFamily::SansSerif, 20).into_font(),
        )
        .margin(20)
        .set_label_area_size(LabelAreaPosition::Bottom, 40)
        .set_label_area_size(LabelAreaPosition::Left, 80)
        .build_cartesian_2d(0..data.len() - 1, 0f64..2f64.powf(10f64))?;
    chart
        .configure_mesh()
        .x_desc(impairment)
        .y_desc("speed")
        .x_label_formatter(&|x| quantities[*x].0.clone())
        .y_label_formatter(&|y| format!("{y}mb/s"))
        .draw()?;
    chart
        .draw_series(LineSeries::new(data.into_iter().enumerate(), &RED))?
        .label("measured")
        .legend(|(x, y)| Rectangle::new([(x, y - 5), (x + 10, y + 5)], RED.filled()));
    chart
        .draw_series(LineSeries::new(
            quantities.into_iter().filter_map(|(_, e)| e).enumerate(),
            &BLUE,
        ))?
        .label("expected")
        .legend(|(x, y)| Rectangle::new([(x, y - 5), (x + 10, y + 5)], BLUE.filled()));
    chart
        .configure_series_labels()
        .border_style(&BLACK)
        .draw()?;
    root.present()?;
    Ok(())
}

fn main() {
    let _ = clean();
    init().unwrap();
    // simulate(
    //     "delay",
    //     (0..11).map(|i| {
    //         (
    //             format!("{}ms", i * 10),
    //             Some((100f64 / i as f64).min(2f64.powf(10f64))), // ((i * 10 / 1000))
    //         )
    //     }),
    // )
    // .unwrap();
    // simulate(
    //     "loss",
    //     (0..11).map(|i| {
    //         (
    //             format!("{}%", i),
    //             Some(2f64.powf(10f64) / (i as f64).sqrt()), // (2 ^ 10 / sqrt(i))
    //         )
    //     }),
    // )
    // .unwrap();
    // simulate(
    //     "duplicate",
    //     (0..11).map(|i| {
    //         (
    //             format!("{}%", i),
    //             Some(1024f64 / (1f64 + i as f64 / 100f64)),
    //         )
    //     }),
    // )
    // .unwrap();
    simulate(
        "delay 10ms reorder",
        (0..11).map(|i| (format!("{:2.1}%", i as f64 / 5f64), None)),
    )
    .unwrap();
    clean().unwrap();
}
