```sh
sudo bash -c "echo '1' > /sys/module/tcp_cubic/parameters/fast_convergence" # enable fast convergence
sudo bash -c "echo '0' > /sys/module/tcp_cubic/parameters/tcp_friendliness" # disable tcp friendliness (just in case)
cargo run --release
```