use anyhow::{Context, Error, Result};
use csv::{Reader, StringRecord, Writer};
use initial::{init_old, simulate_old};
use nix::libc::{getsockopt, SOL_TCP, TCP_INFO};
use plotters::prelude::*;
use revised::{init_new, simulate_new};
use std::{
    mem::{size_of, MaybeUninit},
    net::TcpStream,
    os::fd::AsRawFd,
    process::Command,
};

mod initial;
mod revised;

pub fn exec(command: impl AsRef<str>, netns: Option<&str>) -> Result<String> {
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

#[repr(C)]
struct TcpInfo {
    pub tcpi_ca_state: u8,
    pub tcpi_state: u8,
    pub tcpi_retransmits: u8,
    pub tcpi_probes: u8,
    pub tcpi_backoff: u8,
    pub tcpi_options: u8,
    pub tcpi_snd_wscale: u8,
    pub tcpi_rcv_wscale: u8,
    pub tcpi_rto: u32,
    pub tcpi_ato: u32,
    pub tcpi_snd_mss: u32,
    pub tcpi_rcv_mss: u32,
    pub tcpi_unacked: u32,
    pub tcpi_sacked: u32,
    pub tcpi_lost: u32,
    pub tcpi_retrans: u32,
    pub tcpi_fackets: u32,
    pub tcpi_last_data_sent: u32,
    pub tcpi_last_ack_sent: u32,
    pub tcpi_last_data_recv: u32,
    pub tcpi_last_ack_recv: u32,
    pub tcpi_pmtu: u32,
    pub tcpi_rcv_ssthresh: u32,
    pub tcpi_rtt: u32,
    pub tcpi_rttvar: u32,
    pub tcpi_snd_ssthresh: u32,
    pub tcpi_snd_cwnd: u32,
    pub tcpi_advmss: u32,
    pub tcpi_reordering: u32,
    pub tcpi_rcv_rtt: u32,
    pub tcpi_rcv_space: u32,
    pub tcpi_total_retrans: u32,
}

fn tcp_info(stream: &TcpStream) -> Result<TcpInfo> {
    unsafe {
        let mut tcp_info = MaybeUninit::<TcpInfo>::uninit();
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

#[derive(Clone, Copy)]
pub struct Measurement {
    bytes_transferred: usize,
    congestion_window: usize,
}

fn estimate(r: f64, p: f64) -> f64 {
    const C: f64 = 0.4;
    f64::max(
        (C * 3.7 / 1.2).powf(0.25) * (r / p).powf(0.75),
        (3.0 / 2.0 / p).powf(0.5),
    )
}

fn plot(r: f64, p: f64, measurements: &[Measurement]) -> Result<()> {
    let path = format!("out/{r}_{:.5}.png", p);
    let root = BitMapBackend::new(&path, (1440, 720)).into_drawing_area();
    root.fill(&WHITE)?;
    let mut chart = ChartBuilder::on(&root)
        .caption(
            format!("CWND (Packet) vs Time (RTT) (r = {r}, p = {:.5})", p),
            ("sans-serif", 32),
        )
        .x_label_area_size(64)
        .y_label_area_size(96)
        .margin_right(32)
        .build_cartesian_2d(0..measurements.len(), 0..4096usize)?;
    chart
        .configure_mesh()
        .x_desc("Time (RTT)")
        .y_desc("CWND (Packet)")
        .label_style(("sans-serif", 24))
        .draw()?;
    chart
        .draw_series(LineSeries::new(
            measurements.iter().map(|e| e.congestion_window).enumerate(),
            RED,
        ))?
        .label("Simulated")
        .legend(|(x, y)| PathElement::new([(x, y), (x + 24, y)], RED));
    let estimation = estimate(r, p) as usize;
    chart
        .draw_series(LineSeries::new(
            [(0, estimation), (measurements.len(), estimation)],
            BLUE,
        ))?
        .label("Estimated")
        .legend(|(x, y)| PathElement::new([(x, y), (x + 24, y)], BLUE));
    chart
        .configure_series_labels()
        .label_font(("sans-serif", 24))
        .border_style(BLACK)
        .draw()?;
    root.present()?;
    Ok(())
}

fn save(r: f64, p: f64, measurements: &[Measurement]) -> Result<()> {
    let mut writer = Writer::from_path(format!("out/{r}_{:.5}.csv", p))?;
    writer.write_record(["bytes_transferred", "congestion_window"])?;
    for measurement in measurements {
        writer.write_record([
            measurement.bytes_transferred.to_string(),
            measurement.congestion_window.to_string(),
        ])?;
    }
    writer.flush()?;
    Ok(())
}

fn load(r: f64, p: f64) -> Result<Vec<Measurement>> {
    let mut measurements = Vec::new();
    let mut reader = Reader::from_path(format!("out/{r}_{:.5}.csv", p))?;
    let mut record = StringRecord::new();
    while reader.read_record(&mut record)? {
        measurements.push(Measurement {
            bytes_transferred: record
                .get(0)
                .context("not enough items in record")?
                .parse()?,
            congestion_window: record
                .get(1)
                .context("not enough items in record")?
                .parse()?,
        })
    }
    Ok(measurements)
}

fn main() {
    init_old().unwrap(); // change to old as needed
    let throughputs = (1..10).map(|i| {
        let r = 10f64.powf(-1.0);
        let p = 10f64.powf(-5.0) * i as f64;
        let measurements = load(r, p).unwrap_or_else(|_| {
            let measurements = simulate_old(r, p).unwrap(); // change to old as needed
            save(r, p, &measurements).unwrap();
            measurements
        });
        plot(r, p, &measurements).unwrap();
        let a = measurements[measurements.len() / 2 - 1];
        let b = measurements[measurements.len() - 1];
        ((b.bytes_transferred - a.bytes_transferred) * 2) as f64
            / measurements.len() as f64
            / 0.1
            / 1024f64 //  B/s -> KB/s
            / 1024f64 // KB/s -> MB/s
    });
    let root = BitMapBackend::new("out/main.png", (1440, 720)).into_drawing_area();
    root.fill(&WHITE).unwrap();
    let mut chart = ChartBuilder::on(&root)
        .caption(
            format!("Average Throughput (KB/s) vs Packet Loss Rate (%) (r = 0.1)"),
            ("sans-serif", 32),
        )
        .x_label_area_size(64)
        .y_label_area_size(64)
        .margin_right(32)
        .build_cartesian_2d(0.00001..0.00009, 0.0..20.0)
        .unwrap();
    chart
        .configure_mesh()
        .x_desc("Packet Loss Rate (%)")
        .y_desc("Average Throuhgput(MB/s)")
        .label_style(("sans-serif", 24))
        .x_label_formatter(&|x: &f64| format!("{:.3}", x * 100.0))
        .y_label_formatter(&|y: &f64| format!("{:.0}", y))
        .draw()
        .unwrap();
    chart
        .draw_series(LineSeries::new(
            throughputs
                .into_iter()
                .enumerate()
                .map(|(i, e)| (0.00001 * (i + 1) as f64, e)),
            RED,
        ))
        .unwrap()
        .label("Simulated")
        .legend(|(x, y)| PathElement::new([(x, y), (x + 24, y)], RED));
    chart
        .draw_series(LineSeries::new(
            (1..10).map(|i| {
                let i = 0.00001 * i as f64;
                (i, estimate(0.1, i) / 0.1 * 1560.0 / 1024.0 / 1024.0)
            }),
            BLUE,
        ))
        .unwrap()
        .label("Estimated")
        .legend(|(x, y)| PathElement::new([(x, y), (x + 24, y)], BLUE));
    chart
        .configure_series_labels()
        .label_font(("sans-serif", 24))
        .border_style(BLACK)
        .draw()
        .unwrap();
    root.present().unwrap();
}
