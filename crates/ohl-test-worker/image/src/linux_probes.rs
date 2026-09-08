//! Raw Linux probes deliberately exercise the kernel policy independently of
//! std's choice of syscall. Unsafe sites pass valid local buffers and literals;
//! denied operations must be killed before side effects. No raw syscall is
//! used by the production parser transport.
use core::arch::asm;

unsafe fn syscall(number: isize, args: [isize; 6]) -> isize {
    let result;
    // SAFETY: the caller supplies valid syscall arguments, including any
    // pointed-to storage. The asm declares all Linux x86-64 ABI clobbers and
    // retains its implicit memory clobber.
    unsafe {
        asm!("syscall", inlateout("rax") number => result,
            in("rdi") args[0], in("rsi") args[1], in("rdx") args[2],
            in("r10") args[3], in("r8") args[4], in("r9") args[5],
            lateout("rcx") _, lateout("r11") _, options(nostack));
    }
    result
}

pub fn crash() -> ! {
    // SAFETY: intentionally raises SIGILL; no memory is touched.
    unsafe { asm!("ud2", options(noreturn, nostack)) }
}

pub fn denied(probe: u8) -> i32 {
    let mut futex_word = 0u32;
    let argv = [c"/bin/true".as_ptr(), core::ptr::null()];
    let envp = [core::ptr::null::<i8>()];
    // SAFETY: all pointer arguments refer to valid local data. If the filter
    // regresses, kernel errors or successful returns are surfaced as a normal
    // failure exit, which the host distinguishes from the required SIGSYS.
    unsafe {
        match probe {
            0 => syscall(257, [-100, c"/etc/hosts".as_ptr() as isize, 0, 0, 0, 0]),
            1 => syscall(
                257,
                [
                    -100,
                    c"/tmp/ohl-denial-probe".as_ptr() as isize,
                    0xc1,
                    0o600,
                    0,
                    0,
                ],
            ),
            2 => syscall(41, [2, 1, 0, 0, 0, 0]), // AF_INET, SOCK_STREAM
            3 => syscall(56, [17, 0, 0, 0, 0, 0]), // clone(SIGCHLD)
            4 => syscall(57, [0; 6]),             // fork
            5 => syscall(
                59,
                [
                    c"/bin/true".as_ptr() as isize,
                    argv.as_ptr() as isize,
                    envp.as_ptr() as isize,
                    0,
                    0,
                    0,
                ],
            ),
            6 => syscall(9, [0, 4096, 7, 0x22, -1, 0]), // RWX anonymous
            7 => {
                let mapping = syscall(9, [0, 4096, 3, 0x22, -1, 0]);
                if mapping < 0 {
                    return 90;
                }
                syscall(10, [mapping, 4096, 5, 0, 0, 0]) // RX promotion
            }
            8 => syscall(9, [0, 4096, 1, 2, 0, 0]), // file-backed
            9 => syscall(72, [3, 0, 0, 0, 0, 0]),   // F_DUPFD
            10 => syscall(158, [0x1001, 0, 0, 0, 0, 0]), // ARCH_SET_GS
            11 => syscall(16, [3, 0x541b, (&raw mut futex_word) as isize, 0, 0, 0]), // FIONREAD
            12 => syscall(202, [(&raw mut futex_word) as isize, 1, 1, 0, 0, 0]), // shared FUTEX_WAKE
            13 => syscall(
                322,
                [
                    3,
                    c"".as_ptr() as isize,
                    argv.as_ptr() as isize,
                    envp.as_ptr() as isize,
                    0x1000,
                    0,
                ],
            ),
            _ => return 90,
        };
    }
    90
}

/// Poll all numbers in bounded batches, without adding metadata syscalls to
/// the policy solely for a fixture. Closed numbers report POLLNVAL.
pub fn fd_inventory() -> u64 {
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct PollFd {
        fd: i32,
        events: i16,
        revents: i16,
    }
    let mut mask = 0;
    for base in (0..64).step_by(8) {
        let mut descriptors = [PollFd {
            fd: 0,
            events: 0,
            revents: 0,
        }; 8];
        for (index, descriptor) in descriptors.iter_mut().enumerate() {
            descriptor.fd = base + index as i32;
        }
        // SAFETY: poll writes only into the eight live descriptors; timeout
        // zero prevents blocking. RLIMIT_NOFILE is eight.
        assert!(unsafe { syscall(7, [descriptors.as_mut_ptr() as isize, 8, 0, 0, 0, 0]) } >= 0);
        for descriptor in descriptors {
            if descriptor.revents & 0x20 == 0 {
                mask |= 1 << descriptor.fd;
            }
        }
    }
    mask
}
