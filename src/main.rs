use std::net::{TcpListener, TcpStream};
use std::io::{self, Read, Write};
use std::os::unix::io::AsRawFd;
use std::collections::HashMap;
use std::ptr;

const BUFFER_SIZE: usize = 4096;
const MAX_EVENTS: usize = 64;

struct Connection {
    stream: TcpStream,
    buffer: Vec<u8>,
    id: usize,
}

struct Server {
    listener: TcpListener,
    port: u16,
    epoll_fd: i32,
    connections: HashMap<i32, Connection>,
    next_id: usize,
}

impl Server {
    pub fn new(port: u16) -> io::Result<Server> {
        let listener = TcpListener::bind(format!("127.0.0.1:{}", port))?;
        listener.set_nonblocking(true)?;
        
        // Create epoll instance
        let epoll_fd = unsafe {
            libc::epoll_create1(libc::EPOLL_CLOEXEC)
        };
        
        if epoll_fd < 0 {
            return Err(io::Error::last_os_error());
        }
        
        let server = Server {
            listener,
            port,
            epoll_fd,
            connections: HashMap::new(),
            next_id: 0,
        };
        
        // Register listener with epoll
        server.register_fd(listener.as_raw_fd(), true)?;
        
        println!("Server started on http://localhost:{}/", port);
        
        Ok(server)
    }
    
    fn register_fd(&self, fd: i32, is_listener: bool) -> io::Result<()> {
        let mut event: libc::epoll_event = unsafe { std::mem::zeroed() };
        event.events = (libc::EPOLLIN | libc::EPOLLOUT) as u32;
        event.u64 = if is_listener { 0 } else { fd as u64 };
        
        let ret = unsafe {
            libc::epoll_ctl(self.epoll_fd, libc::EPOLL_CTL_ADD, fd, &mut event)
        };
        
        if ret < 0 {
            return Err(io::Error::last_os_error());
        }
        
        Ok(())
    }
    
    fn unregister_fd(&self, fd: i32) -> io::Result<()> {
        let ret = unsafe {
            libc::epoll_ctl(self.epoll_fd, libc::EPOLL_CTL_DEL, fd, ptr::null_mut())
        };
        
        if ret < 0 {
            return Err(io::Error::last_os_error());
        }
        
        Ok(())
    }
    
    pub fn run(&mut self) -> io::Result<()> {
        let mut events: Vec<libc::epoll_event> = vec![unsafe { std::mem::zeroed() }; MAX_EVENTS];
        
        loop {
            let nfds = unsafe {
                libc::epoll_wait(self.epoll_fd, events.as_mut_ptr(), MAX_EVENTS as i32, -1)
            };
            
            if nfds < 0 {
                return Err(io::Error::last_os_error());
            }
            
            let listener_fd = self.listener.as_raw_fd();
            let mut to_remove = Vec::new();
            
            for i in 0..nfds as usize {
                let event = events[i];
                let fd = unsafe { event.u64 } as i32;
                
                // Check if it's the listener socket
                if fd == 0 && (event.events & libc::EPOLLIN as u32) != 0 {
                    // Accept new connections
                    loop {
                        match self.listener.accept() {
                            Ok((stream, addr)) => {
                                println!("New connection from: {}", addr);
                                stream.set_nonblocking(true)?;
                                
                                let stream_fd = stream.as_raw_fd();
                                let connection = Connection {
                                    stream,
                                    buffer: Vec::with_capacity(BUFFER_SIZE),
                                    id: self.next_id,
                                };
                                
                                self.next_id += 1;
                                self.connections.insert(stream_fd, connection);
                                self.register_fd(stream_fd, false)?;
                            }
                            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                                break;
                            }
                            Err(e) => {
                                eprintln!("Error accepting connection: {}", e);
                                break;
                            }
                        }
                    }
                } else if let Some(connection) = self.connections.get_mut(&fd) {
                    // Handle client data
                    if (event.events & libc::EPOLLIN as u32) != 0 {
                        let mut buf = [0; BUFFER_SIZE];
                        match connection.stream.read(&mut buf) {
                            Ok(0) => {
                                println!("Connection {} closed by client", connection.id);
                                to_remove.push(fd);
                            }
                            Ok(n) => {
                                if let Ok(request) = String::from_utf8(buf[..n].to_vec()) {
                                    println!("Received request:\n{}", request);
                                    self.send_response(&mut connection.stream)?;
                                }
                            }
                            Err(e) if e.kind() != io::ErrorKind::WouldBlock => {
                                eprintln!("Error reading from client {}: {}", connection.id, e);
                                to_remove.push(fd);
                            }
                            Err(_) => {}
                        }
                    }
                }
            }
            
            // Remove closed connections
            for fd in to_remove {
                if let Some(connection) = self.connections.remove(&fd) {
                    let _ = self.unregister_fd(fd);
                    println!("Connection {} removed", connection.id);
                }
            }
        }
    }
    
    fn send_response(&self, stream: &mut TcpStream) -> io::Result<()> {
        let response = format!(
            "HTTP/1.1 200 OK\r\n\
             Content-Type: text/html\r\n\
             Content-Length: 103\r\n\
             \r\n\
             <html><body><h1>Hello from Rust Server on port {}!</h1><p>Your request was received.</p></body></html>",
            self.port
        );
        
        stream.write_all(response.as_bytes())?;
        stream.flush()?;
        Ok(())
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        unsafe {
            libc::close(self.epoll_fd);
        }
    }
}

fn main() -> io::Result<()> {
    let mut server = Server::new(8080)?;
    server.run()
}