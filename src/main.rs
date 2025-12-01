use std::net::{TcpListener, TcpStream};
use std::io::{self, Read, Write};
use std::os::unix::io::{AsRawFd, RawFd};
use std::collections::HashMap;
use libc::{epoll_create1, epoll_ctl, epoll_wait, epoll_event, EPOLLIN, EPOLLERR, EPOLLHUP, EPOLL_CTL_ADD, EPOLL_CTL_DEL};

const MAX_EVENTS: usize = 1024;
const TIMEOUT_MS: i32 = 1000; // 1 second timeout

struct Connection {
    stream: TcpStream,
}

struct Server {
    listener: TcpListener,
    port: u16,
    epoll_fd: RawFd,
    connections: HashMap<RawFd, Connection>,
}

impl Server {
    pub fn new(port: u16) -> io::Result<Server> {
        let listener = TcpListener::bind(format!("127.0.0.1:{}", port))?;
        listener.set_nonblocking(true)?;
        
        // Create epoll instance
        let epoll_fd = unsafe { epoll_create1(0) };
        if epoll_fd < 0 {
            return Err(io::Error::last_os_error());
        }

        // Add listener to epoll
        let mut event = epoll_event {
            events: EPOLLIN as u32,
            u64: listener.as_raw_fd() as u64,
        };

        unsafe {
            if epoll_ctl(
                epoll_fd,
                EPOLL_CTL_ADD,
                listener.as_raw_fd(),
                &mut event as *mut epoll_event,
            ) < 0 {
                return Err(io::Error::last_os_error());
            }
        }
        
        println!("Server started on http://localhost:{}/", port);
        
        Ok(Server {
            listener,
            port,
            epoll_fd,
            connections: HashMap::new(),
        })
    }
    
    pub fn run(&mut self) -> io::Result<()> {
        let mut events = vec![epoll_event { events: 0, u64: 0 }; MAX_EVENTS];
        
        loop {
            let num_events = unsafe {
                epoll_wait(
                    self.epoll_fd,
                    events.as_mut_ptr(),
                    MAX_EVENTS as i32,
                    TIMEOUT_MS,
                )
            };

            if num_events < 0 {
                return Err(io::Error::last_os_error());
            }

            for i in 0..num_events as usize {
                let fd = events[i].u64 as RawFd;

                if fd == self.listener.as_raw_fd() {
                    // Handle new connection
                    self.accept_connection()?;
                } else {
                    // Handle existing connection
                    if events[i].events & (EPOLLERR as u32 | EPOLLHUP as u32) != 0 {
                        self.remove_connection(fd)?;
                        continue;
                    }

                    if events[i].events & EPOLLIN as u32 != 0 {
                        if let Err(_) = self.handle_client_data(fd) {
                            self.remove_connection(fd)?;
                        }
                    }
                }
            }
        }
    }

    fn accept_connection(&mut self) -> io::Result<()> {
        match self.listener.accept() {
            Ok((stream, addr)) => {
                println!("New connection from: {}", addr);
                stream.set_nonblocking(true)?;
                
                let fd = stream.as_raw_fd();
                let mut event = epoll_event {
                    events: EPOLLIN as u32,
                    u64: fd as u64,
                };

                unsafe {
                    if epoll_ctl(self.epoll_fd, EPOLL_CTL_ADD, fd, &mut event as *mut epoll_event) < 0 {
                        return Err(io::Error::last_os_error());
                    }
                }

                self.connections.insert(fd, Connection {
                    stream,
                });
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
            Err(e) => eprintln!("Error accepting connection: {}", e),
        }
        Ok(())
    }

    fn remove_connection(&mut self, fd: RawFd) -> io::Result<()> {
        unsafe {
            epoll_ctl(self.epoll_fd, EPOLL_CTL_DEL, fd, std::ptr::null_mut());
        }
        self.connections.remove(&fd);
        Ok(())
    }

    fn handle_client_data(&mut self, fd: RawFd) -> io::Result<()> {
        let should_send_response = if let Some(connection) = self.connections.get_mut(&fd) {
            let mut buffer = [0; 4096];
            match connection.stream.read(&mut buffer) {
                Ok(0) => {
                    // Connection closed by client
                    println!("Connection closed by client");
                    return Err(io::Error::new(io::ErrorKind::Other, "Connection closed"));
                }
                Ok(n) => {
                    if let Ok(request) = String::from_utf8(buffer[..n].to_vec()) {
                        println!("Received request:\n{}", request);
                        true
                    } else {
                        false
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    return Ok(());
                }
                Err(e) => {
                    eprintln!("Error reading from client: {}", e);
                    return Err(e);
                }
            }
        } else {
            return Ok(());
        };

        if should_send_response {
            if let Some(conn) = self.connections.get_mut(&fd) {
                Self::send_response(&mut conn.stream, self.port)?;
            }
        }
        Ok(())
    }
    
    fn send_response(stream: &mut TcpStream, port: u16) -> io::Result<()> {
        let response = format!(
            "HTTP/1.1 200 OK\r\n\
             Content-Type: text/html\r\n\
             Content-Length: 98\r\n\
             \r\n\
             <html><body><h1>Hello from Rust Server on port {}!</h1><p>Your request was received.</p></body></html>",
            port
        );
        
        stream.write_all(response.as_bytes())?;
        stream.flush()?;
        Ok(())
    }
}

fn main() -> io::Result<()> {
    let mut server = Server::new(8080)?;
    server.run()
}