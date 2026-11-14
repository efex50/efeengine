use super::{thread::{Ctx, TickSignal}, ProgramRuntime, RunResult};

impl ProgramRuntime{

    /// runs the program until Finish, the end of the program or a fault
    pub fn run(&mut self) -> RunResult{
        self.run_with_limit(u64::MAX)
    }

    /// like `run`, but stops after `max_instructions` instructions with `LimitReached`
    pub fn run_with_limit(&mut self, max_instructions: u64) -> RunResult{
        let mut executed: u64 = 0;
        if self.threads.is_empty() {
            return RunResult::EndOfProgram;
        }
        loop {
            for t in 0..self.threads.len() {
                if executed >= max_instructions {
                    return RunResult::LimitReached;
                }
                executed += 1;
                match self.tick(t) {
                    TickSignal::Continue => (),
                    TickSignal::End => return RunResult::EndOfProgram,
                    TickSignal::Finish(code) => return RunResult::Finished(code),
                    TickSignal::Fault(msg) => {
                        eprintln!("efescript: runtime error: {msg}");
                        return RunResult::Fault(msg);
                    }
                }
            }
        }
    }

    /// runs the program until end
    pub fn run_all_nonstop(&mut self){
        self.run();
    }

    /// ticks all the threads
    pub fn tick_all(&mut self) -> Vec<TickSignal> {
        (0..self.threads.len()).map(|t| self.tick(t)).collect()
    }

    /// ticks selected thread
    pub fn tick(&mut self,thread:usize) -> TickSignal{
        let mut ctx = Ctx{ page: &mut self.program, host: &mut *self.host };
        match self.threads.get_mut(thread) {
            Some(t) => t.tick(&mut ctx),
            None => TickSignal::Fault(format!("no thread {thread}")),
        }
    }
}
