//! The readings behind the live figures: what one look at the machine returns, and on macOS the calls into the kernel and
//! IOKit that fill it in. The processor and network figures are the difference between two looks (see `sysstats`).

/// One look at the machine; the processor and network figures are the difference between two looks.
#[derive(Debug, Default)]
pub struct Reading {
    /// Ticks the processors spent in user, system, idle and nice, over all cores together (32-bit: they wrap).
    pub ticks: [u32; 4],
    /// Memory in use, 0..1.
    pub mem: f32,
    pub gpu: f32,
    /// (interface index, bytes in + out so far) of each en* interface.
    pub nets: Vec<(u16, u64)>,
}

#[cfg(target_os = "macos")]
pub use mac::Meter;

/// Nothing to read from off macOS: the figures stay at zero.
#[cfg(not(target_os = "macos"))]
pub struct Meter;

#[cfg(not(target_os = "macos"))]
impl Meter {
    pub fn new() -> Meter {
        Meter
    }
    pub fn read(&mut self) -> Reading {
        Reading::default()
    }
}

/// The interface list `sysctl(NET_RT_IFLIST2)` returns, read by hand. It carries each interface's 64-bit byte counters (what
/// `netstat -ib` prints); `getifaddrs` has only 32-bit ones, which wrap at 4 GB.
#[cfg(any(target_os = "macos", test))]
mod ifinfo {
    /// RTM_IFINFO2: the message with an interface's counters (the list has address messages too).
    const IFINFO2: u8 = 0x12;
    /// RTA_IFP in `ifm_addrs`: the interface's link-level address, which holds its name, follows the header.
    const RTA_IFP: i32 = 0x10;
    /// `if_msghdr2`: its size and where the address bits, the interface index and the bytes in and out sit.
    const HDR: usize = 160;
    const ADDRS: usize = 4;
    const INDEX: usize = 12;
    const IBYTES: usize = 96;
    const OBYTES: usize = 104;
    /// `sockaddr_dl`, after the header: where the length of the name and the name sit.
    const NLEN: usize = 5;
    const NAME: usize = 8;

    // the offsets above, against the layout libc has for them
    #[cfg(target_os = "macos")]
    const _: () = {
        use std::mem::{offset_of, size_of};
        assert!(IFINFO2 as i32 == libc::RTM_IFINFO2);
        assert!(size_of::<libc::if_msghdr2>() == HDR);
        assert!(offset_of!(libc::if_msghdr2, ifm_addrs) == ADDRS);
        assert!(offset_of!(libc::if_msghdr2, ifm_index) == INDEX);
        assert!(offset_of!(libc::if_msghdr2, ifm_data.ifi_ibytes) == IBYTES);
        assert!(offset_of!(libc::if_msghdr2, ifm_data.ifi_obytes) == OBYTES);
        assert!(offset_of!(libc::sockaddr_dl, sdl_nlen) == NLEN);
        assert!(offset_of!(libc::sockaddr_dl, sdl_data) == NAME);
    };

    /// (interface index, bytes in + out) of each en* interface in a list; whatever doesn't parse is skipped.
    pub fn en_bytes(list: &[u8]) -> Vec<(u16, u64)> {
        let mut found = Vec::new();
        let mut rest = list;
        // every message starts with its length (u16), a version and its type (u8 each)
        while rest.len() >= 4 {
            let len = u16::from_ne_bytes([rest[0], rest[1]]) as usize;
            if len < 4 || len > rest.len() {
                break;
            }
            let (m, tail) = rest.split_at(len);
            rest = tail;
            if m[3] != IFINFO2 || len < HDR + NAME {
                continue;
            }
            let Some(name) = m.get(HDR + NAME..HDR + NAME + m[HDR + NLEN] as usize) else { continue };
            let addrs = i32::from_ne_bytes([m[ADDRS], m[ADDRS + 1], m[ADDRS + 2], m[ADDRS + 3]]);
            if addrs & RTA_IFP != 0 && name.starts_with(b"en") {
                let bytes = |at: usize| u64::from_ne_bytes(m[at..at + 8].try_into().unwrap());
                found.push((u16::from_ne_bytes([m[INDEX], m[INDEX + 1]]), bytes(IBYTES).saturating_add(bytes(OBYTES))));
            }
        }
        found
    }
}

/// The macOS readers. Every figure comes from a call into the kernel or IOKit; nothing is parsed from a tool's output.
#[cfg(target_os = "macos")]
mod mac {
    use super::{ifinfo, Reading};
    use libc::{c_char, c_void};

    /// A CoreFoundation object (string, dictionary or number) by pointer.
    type CFRef = *const c_void;
    /// A Mach port name; IOKit's iterators and registry entries are these too.
    type Port = libc::mach_port_t;
    /// kCFStringEncodingUTF8 and kCFNumberSInt64Type.
    const UTF8: u32 = 0x0800_0100;
    const SINT64: isize = 4;

    #[link(name = "IOKit", kind = "framework")]
    extern "C" {
        fn IOServiceMatching(name: *const c_char) -> CFRef;
        fn IOServiceGetMatchingServices(main_port: Port, matching: CFRef, existing: *mut Port) -> libc::kern_return_t;
        fn IOIteratorNext(iterator: Port) -> Port;
        fn IORegistryEntryCreateCFProperty(entry: Port, key: CFRef, allocator: CFRef, options: u32) -> CFRef;
        fn IOObjectRelease(object: Port) -> libc::kern_return_t;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFStringCreateWithCString(allocator: CFRef, c_str: *const c_char, encoding: u32) -> CFRef;
        fn CFDictionaryGetValue(dict: CFRef, key: CFRef) -> CFRef;
        fn CFNumberGetValue(number: CFRef, kind: isize, out: *mut c_void) -> u8;
        fn CFGetTypeID(cf: CFRef) -> usize;
        fn CFDictionaryGetTypeID() -> usize;
        fn CFNumberGetTypeID() -> usize;
        fn CFRelease(cf: CFRef);
    }

    extern "C" {
        fn mach_host_self() -> Port;
        fn host_page_size(host: Port, size: *mut libc::vm_size_t) -> libc::kern_return_t;
    }

    /// What the readers keep between looks: the host port (asked for once, every `mach_host_self` call adds a reference to it),
    /// physical memory and the page size the VM counts are in, the GPU's lookup strings, and a buffer for the network list.
    pub struct Meter {
        host: Port,
        pub memsize: f64,
        page: f64,
        gpu: Gpu,
        buf: Vec<u8>,
    }

    impl Meter {
        pub fn new() -> Meter {
            // SAFETY: mach_host_self takes nothing; sysctlbyname and host_page_size each write one integer into a local of the size they are told.
            unsafe {
                let host = mach_host_self();
                let mut mem = 0u64;
                let mut len = std::mem::size_of::<u64>();
                let ok = libc::sysctlbyname(c"hw.memsize".as_ptr(), &mut mem as *mut u64 as *mut c_void, &mut len, std::ptr::null_mut(), 0) == 0;
                let mut page: libc::vm_size_t = 0;
                let known = host_page_size(host, &mut page) == libc::KERN_SUCCESS && page > 0;
                Meter { host, memsize: if ok { mem as f64 } else { 0.0 }, page: if known { page as f64 } else { 16384.0 }, gpu: Gpu::new(), buf: Vec::new() }
            }
        }

        pub fn read(&mut self) -> Reading {
            Reading { ticks: self.ticks(), mem: self.mem(), gpu: self.gpu.read(), nets: self.nets() }
        }

        /// User, system, idle and nice ticks of all processors together; zeros if the kernel doesn't answer.
        fn ticks(&self) -> [u32; 4] {
            // SAFETY: host_statistics fills at most `count` integers of the struct it is given, which is that big.
            unsafe {
                let mut info: libc::host_cpu_load_info = std::mem::zeroed();
                let mut count = libc::HOST_CPU_LOAD_INFO_COUNT;
                match libc::host_statistics(self.host, libc::HOST_CPU_LOAD_INFO, &mut info as *mut _ as libc::host_info_t, &mut count) {
                    libc::KERN_SUCCESS => info.cpu_ticks,
                    _ => [0; 4],
                }
            }
        }

        /// Active, wired and compressed pages over physical memory (what Activity Monitor calls memory used, roughly), 0..1.
        fn mem(&self) -> f32 {
            // SAFETY: host_statistics64 fills at most `count` integers of the struct it is given, which is that big.
            let vm = unsafe {
                let mut vm: libc::vm_statistics64 = std::mem::zeroed();
                let mut count = libc::HOST_VM_INFO64_COUNT;
                if libc::host_statistics64(self.host, libc::HOST_VM_INFO64, &mut vm as *mut _ as libc::host_info64_t, &mut count) != libc::KERN_SUCCESS {
                    return 0.0;
                }
                vm
            };
            if self.memsize <= 0.0 {
                return 0.0;
            }
            let used = vm.active_count as f64 + vm.wire_count as f64 + vm.compressor_page_count as f64;
            ((used * self.page) / self.memsize).clamp(0.0, 1.0) as f32
        }

        /// (interface index, bytes in + out) of each en* interface, from the routing table's interface list.
        fn nets(&mut self) -> Vec<(u16, u64)> {
            let mut mib = [libc::CTL_NET, libc::PF_ROUTE, 0, 0, libc::NET_RT_IFLIST2, 0];
            // the list can grow between asking how big it is and reading it: leave room, and try again if it still doesn't fit
            for _ in 0..3 {
                let mut len = 0usize;
                // SAFETY: with no buffer sysctl only reports the size it needs; the second call writes at most `len` bytes into a buffer of that size.
                unsafe {
                    if libc::sysctl(mib.as_mut_ptr(), mib.len() as u32, std::ptr::null_mut(), &mut len, std::ptr::null_mut(), 0) != 0 {
                        return Vec::new();
                    }
                    self.buf.resize(len + len / 8, 0);
                    len = self.buf.len();
                    if libc::sysctl(mib.as_mut_ptr(), mib.len() as u32, self.buf.as_mut_ptr().cast(), &mut len, std::ptr::null_mut(), 0) == 0 {
                        return ifinfo::en_bytes(&self.buf[..len]);
                    }
                }
            }
            Vec::new()
        }
    }

    /// The two strings the GPU read looks up, made once (CoreFoundation objects, released on drop).
    struct Gpu {
        stats: CFRef,
        util: CFRef,
    }

    impl Gpu {
        fn new() -> Gpu {
            // SAFETY: CFStringCreateWithCString copies the NUL-terminated bytes it is given (NULL: the default allocator).
            unsafe { Gpu { stats: CFStringCreateWithCString(std::ptr::null(), c"PerformanceStatistics".as_ptr(), UTF8), util: CFStringCreateWithCString(std::ptr::null(), c"Device Utilization %".as_ptr(), UTF8) } }
        }

        /// The GPU's own utilisation figure ("Device Utilization %" in the accelerator's PerformanceStatistics, the first accelerator
        /// that has one), 0..1; 0 when there is none.
        fn read(&self) -> f32 {
            if self.stats.is_null() || self.util.is_null() {
                return 0.0;
            }
            // SAFETY: IOServiceGetMatchingServices consumes the matching dictionary; the iterator, each service it hands out and each properties
            // dictionary (a copy that is ours) are released here; the number inside is only borrowed while its dictionary lives.
            unsafe {
                let mut iter: Port = 0;
                if IOServiceGetMatchingServices(0, IOServiceMatching(c"IOAccelerator".as_ptr()), &mut iter) != libc::KERN_SUCCESS {
                    return 0.0;
                }
                let mut percent = None;
                loop {
                    let service = IOIteratorNext(iter);
                    if service == 0 {
                        break;
                    }
                    let props = IORegistryEntryCreateCFProperty(service, self.stats, std::ptr::null(), 0);
                    IOObjectRelease(service);
                    if props.is_null() {
                        continue;
                    }
                    if CFGetTypeID(props) == CFDictionaryGetTypeID() {
                        let number = CFDictionaryGetValue(props, self.util);
                        if !number.is_null() && CFGetTypeID(number) == CFNumberGetTypeID() {
                            let mut v = 0i64;
                            CFNumberGetValue(number, SINT64, &mut v as *mut i64 as *mut c_void);
                            percent = Some(v);
                        }
                    }
                    CFRelease(props);
                    if percent.is_some() {
                        break;
                    }
                }
                IOObjectRelease(iter);
                percent.map_or(0.0, |v| (v as f32 / 100.0).clamp(0.0, 1.0))
            }
        }
    }

    impl Drop for Gpu {
        fn drop(&mut self) {
            for s in [self.stats, self.util] {
                if !s.is_null() {
                    // SAFETY: made by CFStringCreateWithCString above and released only here.
                    unsafe { CFRelease(s) };
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One interface message as `sysctl(NET_RT_IFLIST2)` lays it out: the 160-byte header, then the link-level address with the name.
    fn if_msg(name: &str, index: u16, bytes_in: u64, bytes_out: u64) -> Vec<u8> {
        let sdl = 8 + name.len() + 6;
        let mut m = vec![0u8; 160 + sdl.div_ceil(4) * 4];
        let len = m.len() as u16;
        m[0..2].copy_from_slice(&len.to_ne_bytes());
        m[2] = 5;
        m[3] = 0x12;
        m[4..8].copy_from_slice(&0x10i32.to_ne_bytes());
        m[12..14].copy_from_slice(&index.to_ne_bytes());
        m[96..104].copy_from_slice(&bytes_in.to_ne_bytes());
        m[104..112].copy_from_slice(&bytes_out.to_ne_bytes());
        m[160] = sdl as u8;
        m[161] = 18;
        m[162..164].copy_from_slice(&index.to_ne_bytes());
        m[165] = name.len() as u8;
        m[166] = 6;
        m[168..168 + name.len()].copy_from_slice(name.as_bytes());
        m
    }

    #[test]
    fn the_interface_list_is_read_for_its_en_interfaces() {
        let addr = {
            // an address message between interfaces: skipped by its type
            let mut m = vec![0u8; 24];
            m[0..2].copy_from_slice(&24u16.to_ne_bytes());
            m[3] = 0x14;
            m
        };
        let mut list = [if_msg("lo0", 1, 7, 7), addr.clone(), if_msg("en0", 6, 5_000_000_000, 100), if_msg("awdl0", 9, 1, 1), addr, if_msg("en5", 14, 1, 2)].concat();
        assert_eq!(ifinfo::en_bytes(&list), vec![(6, 5_000_000_100), (14, 3)]);
        // a message cut short, or one that claims more than is left, ends the walk without a panic
        let cut = list.len() - 5;
        assert_eq!(ifinfo::en_bytes(&list[..cut]), vec![(6, 5_000_000_100)]);
        list.extend([0, 0, 0, 0]);
        assert_eq!(ifinfo::en_bytes(&list).len(), 2);
        assert!(ifinfo::en_bytes(&[]).is_empty());
        assert!(ifinfo::en_bytes(&[9, 9]).is_empty());
        // a name that runs past its message is skipped
        let mut bad = if_msg("en0", 6, 1, 1);
        bad[165] = 200;
        assert!(ifinfo::en_bytes(&bad).is_empty());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_native_readers_give_sane_figures() {
        use crate::sysstats::{busy, net_rate};
        use std::time::Duration;
        let mut meter = Meter::new();
        assert!(meter.memsize > 0.0, "hw.memsize");
        let a = meter.read();
        std::thread::sleep(Duration::from_millis(200));
        let b = meter.read();
        assert!(b.ticks.iter().map(|&t| t as u64).sum::<u64>() > 0, "processor ticks");
        for (what, v) in [("cpu", busy(&a.ticks, &b.ticks)), ("memory", b.mem), ("gpu", b.gpu)] {
            assert!((0.0..=1.0).contains(&v), "{what} {v}");
        }
        assert!(b.mem > 0.0, "some memory is in use");
        let rate = net_rate(&a.nets, &b.nets, 0.2);
        assert!(rate.is_finite() && rate >= 0.0, "{rate}");
        // SAFETY: if_nametoindex reads the NUL-terminated name it is given.
        let en0 = unsafe { libc::if_nametoindex(c"en0".as_ptr()) };
        assert!(en0 == 0 || b.nets.iter().any(|(i, _)| *i as u32 == en0), "en0 is index {en0}, the list has {:?}", b.nets);
    }
}
