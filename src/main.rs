#[macro_use]
extern crate nom;

mod parser;

use parser::*;

mod jobqueue;

use jobqueue::*;

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::str;

struct Server {
    stream: TcpStream,
    job_queue: JobQueue,
}

impl Server {
    fn new(stream: TcpStream, job_queue: JobQueue) -> Server {
        Server {
            stream: stream,
            job_queue: job_queue,
        }
    }

    fn run(&mut self) {
        let mut parser = Parser::new();

        loop {
            if parser.is_incomplete() {
                parser.allocate();
                let len = {
                    let pos = parser.written;
                    let mut buffer = parser.get_mut();

                    // read socket
                    match self.stream.read(&mut buffer[pos..]) {
                        Ok(r) => r,
                        Err(err) => {
                            println!("Reading from client: {:?}", err);
                            break;
                        }
                    }
                };
                parser.written += len;

                // client closed connection
                if len == 0 {
                    println!("Client closed connection");
                    break;
                }
            }

            match parser.next() {
                Ok(command) => {
                    println!("Received command {:?}", command);

                    match command {
                        Command::Put {data} => {
                            let mut alloc_data = Vec::new();
                            alloc_data.extend_from_slice(data);

                            let id = self.job_queue.put(1, 1, 1, alloc_data);

                            let response = format!("INSERTED {}\r\n", id);

                            self.stream.write(response.as_bytes());
                        },
                        Command::Reserve => {
                            let (job_id, job_data) = self.job_queue.reserve();

                            let header = format!("RESERVED {} {}\r\n", job_id, job_data.len());

                            self.stream.write(header.as_bytes());
                            self.stream.write(job_data.as_slice());
                            self.stream.write(b"\r\n");
                        },
                        Command::Delete {id} => {
                            let id = str::from_utf8(id)
                                .unwrap()
                                .parse::<u8>()
                                .unwrap();

                            match self.job_queue.delete(&id) {
                                Some(_) => self.stream.write(b"DELETED\r\n"),
                                None => self.stream.write(b"NOT FOUND\r\n"),
                            };
                        },
                        Command::Release {id, pri, delay} => {
                            let id = str::from_utf8(id)
                                .unwrap()
                                .parse::<u8>()
                                .unwrap();

                            match self.job_queue.release(&id) {
                                Some(_) => self.stream.write(b"RELEASED\r\n"),
                                None => self.stream.write(b"NOT FOUND\r\n"),
                            };
                        },
                    };
                },
                Err(err) => {
                    match err {
                        // if it's incomplete, keep adding to the buffer
                        ParseError::Incomplete => {
                            println!("Incomplete");
                            continue;
                        }
                        _ => {
                            println!("Protocol error from client: {:?}", err);
                            break;
                        }
                    }
                }
            };
        }
    }
}

fn main() {
    let listener = TcpListener::bind("127.0.0.1:11300").unwrap();

    for stream in listener.incoming() {
        match stream {
            Err(_) => panic!("error listen"),
            Ok(stream) => {
                let mut server = Server::new(stream, JobQueue::new());
                server.run();
            },
        };
    }
}
