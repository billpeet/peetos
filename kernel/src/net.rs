use drivers::pci;
use net::{Ipv4Addr, MacAddr, NetConfig};

use crate::serial_println;

static mut NET_CONFIG: NetConfig = NetConfig::new();

pub fn get_config() -> &'static NetConfig {
    unsafe { &NET_CONFIG }
}

pub fn config_set_mac(mac: &MacAddr) {
    unsafe {
        NET_CONFIG.mac.copy_from(mac);
    }
}

pub fn init(pci_devices: &[pci::DeviceInfo]) {
    serial_println!("[NET] Initializing network subsystem...");

    // Initialize E1000 NIC driver
    crate::e1000::init(pci_devices);
    if !crate::e1000::is_initialized() {
        serial_println!("[NET] No NIC found, skipping network init");
        return;
    }

    // Configure static IP (QEMU user-mode NAT defaults)
    unsafe {
        NET_CONFIG.ip = Ipv4Addr::new(10, 0, 2, 15);
        NET_CONFIG.netmask = Ipv4Addr::new(255, 255, 255, 0);
        NET_CONFIG.gateway = Ipv4Addr::new(10, 0, 2, 2);
        NET_CONFIG.configured = true;
    }
    serial_println!("[NET] IP configured: 10.0.2.15/24, gateway 10.0.2.2");

    // Initialize protocol layers
    crate::ethernet::init();
    crate::arp::init();
    crate::ipv4::init();
    crate::icmp::init();
    crate::udp::init();
    crate::tcp::init();

    // Send gratuitous ARP
    crate::arp::send_gratuitous();

    serial_println!("[NET] Network subsystem initialized");

    // Smoke test: ping the QEMU gateway
    #[cfg(feature = "net-ping-smoke")]
    {
        serial_println!("[NET] Ping smoke test: pinging 10.0.2.2...");
        let target = Ipv4Addr::new(10, 0, 2, 2);
        match crate::icmp::ping(&target, 5000) {
            Ok(rtt) => {
                serial_println!("[NET] Ping 10.0.2.2: reply, RTT={} ms", rtt);
            }
            Err(_) => {
                serial_println!("[NET] Ping 10.0.2.2: timeout or error");
            }
        }
    }

    #[cfg(feature = "net-udp-smoke")]
    crate::udp::smoke_test();

    #[cfg(feature = "net-tcp-smoke")]
    crate::tcp::smoke_test();
}

/// Poll for incoming network packets. Call from idle loop.
pub fn poll() {
    if crate::e1000::is_initialized() {
        crate::e1000::poll();
        crate::tcp::tick();
    }
}
