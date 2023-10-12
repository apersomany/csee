use std::{
    mem::{size_of, MaybeUninit},
    net::TcpStream,
    os::fd::AsRawFd,
};

use nix::{
    libc::{getsockopt, SOL_TCP, TCP_INFO},
    Result,
};

#[derive(Debug)]
#[repr(C)]
pub struct TcpInfo {
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

impl TcpInfo {
    pub fn read(stream: &TcpStream) -> Result<Self> {
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
