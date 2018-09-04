#[macro_use] extern crate downcast;
#[macro_use] extern crate prolog_parser;
extern crate termion;

use prolog::ast::*;

mod prolog;

use prolog::compile::*;
use prolog::io::*;
use prolog::machine::*;

#[cfg(test)]
mod tests;

fn parse_and_compile_line(wam: &mut Machine, buffer: &str)
{
    match parse_code(wam, buffer) {
        Ok(packet) => {
            let result = compile_packet(wam, packet);
            print(wam, result);
        },
        Err(s) => println!("{:?}", s)
    }
}

fn prolog_repl() {
    let mut wam = Machine::new();

    loop {
        print!("prolog> ");

        match read() {
            Input::Line(line) => parse_and_compile_line(&mut wam, line.as_str()),
            Input::Batch(batch) =>
                match compile_user_module(&mut wam, batch.as_str()) {
                    EvalSession::Error(e) => println!("{}", e),
                    _ => {}
                },
            Input::Quit => break,
            Input::Clear => {
                wam.clear();
                continue;
            }
        };

        wam.reset();
    }
}

fn main() {
    prolog_repl();
}
