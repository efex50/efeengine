use std::path::Path;

use crate::{prelude::parse_from_file, runtime::host::Host, runtime::program::thread::PThread};

use super::ProgramRuntime;


impl ProgramRuntime{
    /// a runtime that writes to the process stdout
    pub fn new() -> Self{
        Self::default()
    }

    /// a runtime that sends output and sounds to `host`
    pub fn with_host(host:Box<dyn Host>) -> Self{
        Self{ host, ..Self::default() }
    }

    pub fn compile_from_file<S:Into<String>>(path:S) -> Self{
        let v = parse_from_file(path).unwrap();
        let mut slf = Self::default();
        slf.load_from_vec(v);
        slf
    }

    
    pub fn load_from_file<S:Into<String>>(&mut self,a:S){
        let path:String = a.into();
        let path = Path::new(&path);
        let v = std::fs::read(path).unwrap();
        self.load_from_vec(v);
    }


    pub fn load_from_vec(&mut self,a:Vec<u8>){
        self.program_size = a.len();
        self.init_program();
        self.program.write(0, a);
    }

    // ?
    pub fn load_from_vec_new() -> Self{
        todo!()
    }

    pub fn print_program_slice(&mut self,start:usize,len:usize) -> String{
        let v = self.program.read(start, len);
        format!("{:?}",v)
    }


    fn init_program(&mut self){
        let mut main_thread = PThread::default();
        main_thread.registers.rsp = self.program_size + 2;
        main_thread.stack_base = self.program_size + 2;
        main_thread.id = 0;

        self.threads.clear();
        self.threads.push(main_thread);
    }

}
