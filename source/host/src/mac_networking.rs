//! Access to the fixed-port, administrator-installed Mac networking broker.
#[cfg(target_os = "macos")]
mod platform {
    use std::{
        io::{self, Write},
        mem::{self, size_of},
        net::UdpSocket,
        os::{
            fd::{AsRawFd, FromRawFd, OwnedFd},
            unix::{
                fs::{FileTypeExt, MetadataExt},
                net::UnixStream,
            },
        },
        time::Duration,
    };
    const SOCKET: &str = "/var/run/org.onelibrarycompanion.networking.sock";
    const INSTALL: &str = "In the Mac application's OneLibraryCompanion menu, choose Local USB Support, install the package with administrator approval, then quit and reopen OLC.";

    pub fn available() -> bool {
        std::fs::symlink_metadata(SOCKET).is_ok_and(|m| m.uid() == 0 && m.file_type().is_socket())
    }

    fn connect() -> io::Result<UnixStream> {
        if !available() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "the networking component is not installed",
            ));
        }
        let stream = UnixStream::connect(SOCKET)?;
        stream.set_read_timeout(Some(Duration::from_secs(3)))?;
        stream.set_write_timeout(Some(Duration::from_secs(3)))?;
        let (mut uid, mut gid) = (0, 0);
        // The peer credentials come from the kernel, not the response payload.
        if unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) } != 0 || uid != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Untrusted networking helper",
            ));
        }
        Ok(stream)
    }

    fn receive(stream: &UnixStream, port: u16) -> io::Result<UdpSocket> {
        let mut status = [0u8; 4];
        let mut io = libc::iovec {
            iov_base: status.as_mut_ptr().cast(),
            iov_len: status.len(),
        };
        // usize provides cmsghdr alignment; enough space to detect/reject extra handles.
        let mut control = [0usize; 16];
        let mut message: libc::msghdr = unsafe { mem::zeroed() };
        message.msg_iov = &mut io;
        message.msg_iovlen = 1;
        message.msg_control = control.as_mut_ptr().cast();
        message.msg_controllen = size_of_val(&control) as _;
        let got = unsafe { libc::recvmsg(stream.as_raw_fd(), &mut message, libc::MSG_WAITALL) };
        if got < 0 {
            return Err(io::Error::last_os_error());
        }
        let mut handles = Vec::new();
        // All received handles are owned immediately, including handles in invalid replies.
        unsafe {
            let mut c = libc::CMSG_FIRSTHDR(&message);
            while !c.is_null() {
                if (*c).cmsg_level == libc::SOL_SOCKET && (*c).cmsg_type == libc::SCM_RIGHTS {
                    let bytes = ((*c).cmsg_len as usize).saturating_sub(libc::CMSG_LEN(0) as usize);
                    for i in 0..bytes / size_of::<libc::c_int>() {
                        let fd = std::ptr::read_unaligned(
                            libc::CMSG_DATA(c).cast::<libc::c_int>().add(i),
                        );
                        handles.push(OwnedFd::from_raw_fd(fd));
                    }
                }
                c = libc::CMSG_NXTHDR(&message, c);
            }
        }
        if got != 4 || message.msg_flags & (libc::MSG_CTRUNC | libc::MSG_TRUNC) != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Incomplete networking helper reply",
            ));
        }
        let error = u32::from_be_bytes(status) as i32;
        if error != 0 {
            return Err(io::Error::from_raw_os_error(error));
        }
        if handles.len() != 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Networking helper did not supply one socket",
            ));
        }
        let fd = handles.pop().unwrap();
        let mut kind: libc::c_int = 0;
        let mut len = size_of_val(&kind) as libc::socklen_t;
        if unsafe {
            libc::getsockopt(
                fd.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_TYPE,
                (&mut kind as *mut libc::c_int).cast(),
                &mut len,
            )
        } != 0
            || kind != libc::SOCK_DGRAM
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Networking helper supplied the wrong socket type",
            ));
        }
        if unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let socket = UdpSocket::from(fd);
        let address = socket.local_addr()?;
        if !address.is_ipv4() || address.port() != port || !address.ip().is_unspecified() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Networking helper supplied the wrong listening address",
            ));
        }
        socket.set_nonblocking(true)?;
        Ok(socket)
    }

    pub fn acquire() -> Result<UdpSocket, String> {
        let result = (|| {
            let mut stream = connect()?;
            stream.write_all(b"OLC1")?;
            receive(&stream, 111)
        })();
        result.map_err(|e: io::Error| match e.raw_os_error() {
            Some(libc::EADDRINUSE) => "UDP port 111 is already in use by another RPC service or OLC session. Close that session or stop the conflicting service before loading local music.".into(),
            Some(libc::EACCES) => format!("The networking helper does not authorize this OLC build. {INSTALL}"),
            _ => format!("Mac local USB networking is unavailable: {e}. {INSTALL}"),
        })
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        fn send(stream: &UnixStream, fds: &[i32], error: i32) {
            let mut status = (error as u32).to_be_bytes();
            let mut io = libc::iovec {
                iov_base: status.as_mut_ptr().cast(),
                iov_len: 4,
            };
            let mut control = [0usize; 16];
            let mut message: libc::msghdr = unsafe { mem::zeroed() };
            message.msg_iov = &mut io;
            message.msg_iovlen = 1;
            if !fds.is_empty() {
                message.msg_control = control.as_mut_ptr().cast();
                message.msg_controllen = unsafe { libc::CMSG_SPACE(size_of_val(fds) as _) };
                unsafe {
                    let c = libc::CMSG_FIRSTHDR(&message);
                    (*c).cmsg_level = libc::SOL_SOCKET;
                    (*c).cmsg_type = libc::SCM_RIGHTS;
                    (*c).cmsg_len = libc::CMSG_LEN(size_of_val(fds) as _);
                    std::ptr::copy_nonoverlapping(
                        fds.as_ptr().cast::<u8>(),
                        libc::CMSG_DATA(c),
                        size_of_val(fds),
                    );
                }
            }
            assert_eq!(unsafe { libc::sendmsg(stream.as_raw_fd(), &message, 0) }, 4);
        }
        #[test]
        fn transferred_socket_works_and_closes_with_last_owner() {
            let (a, b) = UnixStream::pair().unwrap();
            let original = UdpSocket::bind("0.0.0.0:0").unwrap();
            let port = original.local_addr().unwrap().port();
            send(&a, &[original.as_raw_fd()], 0);
            let received = receive(&b, port).unwrap();
            drop(original);
            assert!(UdpSocket::bind((std::net::Ipv4Addr::UNSPECIFIED, port)).is_err());
            let peer = UdpSocket::bind("127.0.0.1:0").unwrap();
            received
                .send_to(b"OLC", peer.local_addr().unwrap())
                .unwrap();
            peer.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
            let mut buffer = [0; 3];
            assert_eq!(peer.recv(&mut buffer).unwrap(), 3);
            assert_eq!(&buffer, b"OLC");
            assert_ne!(
                unsafe { libc::fcntl(received.as_raw_fd(), libc::F_GETFD) },
                0
            );
            drop(received);
            assert!(UdpSocket::bind((std::net::Ipv4Addr::UNSPECIFIED, port)).is_ok());
        }
        #[test]
        fn reject_missing_extra_wrong_port_and_error_replies() {
            let udp = UdpSocket::bind("0.0.0.0:0").unwrap();
            let port = udp.local_addr().unwrap().port();
            for (fds, error, expected) in [
                (vec![], 0, port),
                (vec![udp.as_raw_fd(); 2], 0, port),
                (vec![udp.as_raw_fd()], 0, 111),
                (vec![], libc::EACCES, port),
            ] {
                let (a, b) = UnixStream::pair().unwrap();
                send(&a, &fds, error);
                assert!(receive(&b, expected).is_err());
            }
            let (a, b) = UnixStream::pair().unwrap();
            send(&a, &[a.as_raw_fd()], 0);
            assert!(receive(&b, port).is_err());
        }
    }
}
#[cfg(target_os = "macos")]
pub use platform::{acquire, available};
#[cfg(not(target_os = "macos"))]
pub fn available() -> bool {
    true
}
