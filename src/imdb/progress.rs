use std::io::Read;

pub struct Progress<'a, R> {
  inner: R,
  progress_fn: &'a dyn Fn(u64),
}

impl<'a, R: Read> Progress<'a, R> {
  pub fn new(inner: R, progress_fn: &'a dyn Fn(u64)) -> Self {
    Self { inner, progress_fn }
  }
}

impl<'a, R: Read> Read for Progress<'a, R> {
  fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
    let bytes = self.inner.read(buf)?;
    (self.progress_fn)(bytes as u64);
    Ok(bytes)
  }
}
