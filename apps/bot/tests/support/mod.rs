use serde_json::Value;
use std::{
    io::Write,
    sync::{Arc, Mutex},
};

#[derive(Clone, Default)]
pub struct Capture(Arc<Mutex<(Vec<u8>, usize)>>);
impl Capture {
    pub fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().0.clone()).unwrap()
    }
    pub fn records(&self) -> Vec<Value> {
        self.text()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
    pub fn flushes(&self) -> usize {
        self.0.lock().unwrap().1
    }
}
impl Write for Capture {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.0.lock().unwrap().1 += 1;
        Ok(())
    }
}
