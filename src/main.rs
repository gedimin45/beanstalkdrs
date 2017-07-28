#[macro_use]
extern crate nom;

use std::collections::HashMap;
use std::iter;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::str;
use nom::{IResult, space, alphanumeric};

#[derive(Debug, PartialEq)]
pub enum ParseError {
    /// The received buffer is valid but needs more data
    Incomplete,
    /// The received buffer is invalid
    BadProtocol(String),
    /// Expected one type of argument and received another
    InvalidArgument,
}

impl ParseError {
    pub fn is_incomplete(&self) -> bool {
        match *self {
            ParseError::Incomplete => true,
            _ => false,
        }
    }
}

named!(beanstalk_request <&[u8], (&[u8], Option<&[u8]>)>,
    do_parse!(
        command: alt!(tag!("put") | tag!("reserve") | tag!("delete")) >>
        opt!(space) >>
        data: opt!(alphanumeric) >>
        tag!("\r\n") >>
        (command, data)
    )
);

fn parse_nom(input: &[u8]) -> Result<(Request, usize), ParseError> {
    match beanstalk_request(input) {
        IResult::Done(i, o) => {
            let command = match o.0 {
                b"put" => Command::Put,
                b"reserve" => Command::Reserve,
                b"delete" => Command::Delete,
                _ => panic!("unknown command")
            };
            Ok((
                Request {command: command, data: o.1},
                input.len()
            ))
        },
        IResult::Incomplete(_) => Err(ParseError::Incomplete),
        IResult::Error(_) => Err(ParseError::InvalidArgument),
    }
}

pub struct Parser {
    data: Vec<u8>,
    pub position: usize,
    pub written: usize,
}

impl Parser {
    pub fn new() -> Parser {
        Parser {
            data: vec![],
            position: 0,
            written: 0,
        }
    }

    pub fn allocate(&mut self) {
        if self.position > 0 && self.written == self.position {
            self.written = 0;
            self.position = 0;
        }

        let len = self.data.len();
        let add = if len == 0 {
            16
        } else if self.written * 2 > len {
            len
        } else {
            0
        };

        if add > 0 {
            self.data.extend(iter::repeat(0).take(add));
        }
    }

    pub fn get_mut(&mut self) -> &mut Vec<u8> {
        &mut self.data
    }

    pub fn is_incomplete(&self) -> bool {
        let data = &(&*self.data)[self.position..self.written];
        match parse_nom(data) {
            Ok(_) => false,
            Err(e) => e.is_incomplete(),
        }
    }

    pub fn next(&mut self) -> Result<Request, ParseError> {
        let data = &(&*self.data)[self.position..self.written];
        let (r, len) = try!(parse_nom(data));
        self.position += len;
        Ok(r)
    }
}

#[derive(Debug)]
enum Command {
    Put,
    Reserve,
    Delete,
}

#[derive(Debug)]
pub struct Request<'a> {
    command: Command,
    data: Option<&'a [u8]>,
}

struct Job {
    id: u8,
    priority: u8,
    delay: u8,
    ttr: u8,
    data: Vec<u8>,
    deleted: bool,
    reserved: bool,
}

struct Server {
    pub queue: HashMap<u8, Job>,
    pub reserved_jobs: HashMap<u8, Job>,
    pub stream: TcpStream,
    pub auto_increment_index: u8,
}

impl Server {
    fn new(stream: TcpStream) -> Server {
        Server {
            queue: HashMap::new(),
            reserved_jobs: HashMap::new(),
            stream: stream,
            auto_increment_index: 0,
        }
    }

    fn put(&mut self, pri: u8, delay: u8, ttr: u8, data: Vec<u8>) -> u8 {
        self.auto_increment_index += 1;
        self.queue.insert(self.auto_increment_index, Job {
            id: self.auto_increment_index,
            priority: pri,
            delay: delay,
            ttr: ttr,
            data: data,
            deleted: false,
            reserved: false,
        });

        self.auto_increment_index
    }

    fn reserve(&mut self) -> (u8, Vec<u8>) {
        let mut items: Vec<(&u8, &mut Job)> = self.queue.iter_mut()
            .filter(|item| !item.1.reserved)
            .take(1)
            .collect();

        match items.pop() {
            Some((id, job)) => {
                job.reserved = true;

                let ret = (*id, job.data.clone());

                self.reserved_jobs.insert(*id, Job {
                    id: job.id,
                    priority: job.priority,
                    delay: job.delay,
                    ttr: job.ttr,
                    data: job.data.clone(),
                    deleted: false,
                    reserved: false,
                });

                ret
            },
            None => panic!("No more jobs!"),
        }
    }

    fn delete(&mut self, id: &u8) -> Option<Job> {
        println!("Deleting job {}", id);
        self.queue.remove(id)
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
                Ok(request) => {
                    println!("Received request {:?}", request);

                    match request.command {
                        Command::Put => {
                            let mut data = Vec::new();
                            data.extend_from_slice(request.data.unwrap());

                            let id = self.put(1, 1, 1, data);

                            let response = format!("INSERTED {}\r\n", id);

                            self.stream.write(response.as_bytes());
                        },
                        Command::Reserve => {
                            let (job_id, job_data) = self.reserve();

                            let header = format!("RESERVED {} {}\r\n", job_id, job_data.len());

                            self.stream.write(header.as_bytes());
                            self.stream.write(job_data.as_slice());
                            self.stream.write(b"\r\n");
                        },
                        Command::Delete => {
                            let id = str::from_utf8(request.data.unwrap())
                                .unwrap()
                                .parse::<u8>()
                                .unwrap();

                            match self.delete(&id) {
                                Some(_) => self.stream.write(b"DELETED\r\n"),
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
                        ParseError::BadProtocol(s) => {
                            println!("Bad protocol {:?}", s);
                            break;
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
                let mut server = Server::new(stream);
                server.run();
            },
        };
    }
}
