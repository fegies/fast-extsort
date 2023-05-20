use std::{
    fs::File,
    io::{self, Read, Seek, Write},
    marker::PhantomData,
    num::NonZeroUsize,
    path::PathBuf,
    process,
};

use crate::run::{file_run::ExternalRun, split_backing::SplitView};

mod file;

pub struct TapeCollection<T> {
    next_file_name: PathBuf,
    max_files: usize,
    phantom: PhantomData<T>,
    plain_tapes: Vec<Tape<File>>,
    shared_tapes: Vec<Tape<SplitView<File>>>,
    next_tape_idx: usize,
}

impl<T> TapeCollection<T> {
    pub fn into_tapes(self, read_buffer_size: NonZeroUsize) -> Vec<ExternalRun<T, Box<dyn Read>>> {
        let mut res = Vec::with_capacity(self.plain_tapes.len() + self.shared_tapes.len() + 1);
        let iter = self
            .plain_tapes
            .into_iter()
            .map(|t| ExternalRun::from_tape(t.box_backing(), read_buffer_size))
            .chain(
                self.shared_tapes
                    .into_iter()
                    .map(|t| ExternalRun::from_tape(t.box_backing(), read_buffer_size)),
            );
        res.extend(iter);
        res
    }
    pub fn new(sort_folder: PathBuf, max_files: NonZeroUsize) -> Self {
        let mut next_file_name = sort_folder;
        next_file_name.push("dummy");
        Self {
            max_files: max_files.into(),
            next_file_name,
            next_tape_idx: 0,
            phantom: PhantomData,
            plain_tapes: Vec::new(),
            shared_tapes: Vec::new(),
        }
    }
    pub fn add_run(&mut self, source: &mut Vec<T>) -> io::Result<()> {
        if self.next_tape_idx >= self.max_files {
            self.add_run_shared(source)?;
        } else {
            self.add_run_simple(source)?;
        }
        self.next_tape_idx += 1;
        Ok(())
    }
    fn add_run_shared(&mut self, source: &mut Vec<T>) -> io::Result<()> {
        let mut new_backing = if let Some(tape) = self.plain_tapes.pop() {
            let mut shared_tape = Tape {
                backing: SplitView::new(tape.backing)?,
                num_entries: tape.num_entries,
            };
            shared_tape.backing.add_segment()?
        } else {
            self.shared_tapes[self.next_tape_idx % self.max_files]
                .backing
                .add_segment()?
        };

        let num_entries = source.len();
        fill_backing(source, &mut new_backing)?;
        self.shared_tapes.push(Tape {
            backing: new_backing.into(),
            num_entries,
        });

        Ok(())
    }

    fn add_run_simple(&mut self, source: &mut Vec<T>) -> io::Result<()> {
        let pid = process::id();
        let self_addr = self as *const Self as usize;
        self.next_file_name.set_file_name(format!(
            "{}_{}_sort_file_{}",
            pid, self_addr, self.next_tape_idx
        ));
        let mut file = file::create_file(&self.next_file_name)?;
        let num_entries = source.len();
        fill_backing(source, &mut file)?;

        // seek to the beginning of the file to ensure that we will actually read its contents
        file.seek(io::SeekFrom::Start(0))?;

        self.plain_tapes.push(Tape {
            num_entries,
            backing: file,
        });
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.plain_tapes.len() == 0 && self.shared_tapes.len() == 0
    }
}

/// Fills the provided file with the values drained from source.
/// When the call completes successfully, source will be empty.
/// If it fails, source will remain untouched.
fn fill_backing<T, TBacking>(source: &mut Vec<T>, file: &mut TBacking) -> io::Result<()>
where
    TBacking: Write,
{
    // we create a byteslice view into the vec
    // SAFETY:
    // this is safe because the alignment restrictions of the byteslice are loose enough to allow this
    // and because if T is zero-sized, we will create an empty slice over T.
    let slice = unsafe {
        let num_bytes = source.len() * std::mem::size_of::<T>();
        std::slice::from_raw_parts(source.as_ptr() as *const u8, num_bytes)
    };

    // move the contents of the vec to the file.
    file.write_all(slice)?;

    // we have conceptually moved all the data that our vec used to contain to disk.
    // in order to make sure that the drop functions are not called twice,
    // we will leak the content of the vec (this is conceptually the same calling mem::forget)
    // on every item in the vec.
    // SAFETY:
    // this is safe because the vec is now empty after this and we no longer refer to
    // any of the elements inside.
    unsafe {
        source.set_len(0);
    }

    Ok(())
}

pub struct Tape<T> {
    num_entries: usize,
    backing: T,
}

impl<T> Tape<T> {
    pub fn num_entries(&self) -> usize {
        self.num_entries
    }
    pub fn into_backing(self) -> T {
        self.backing
    }
}

impl<T: Read + 'static> Tape<T> {
    fn box_backing(self) -> Tape<Box<dyn Read>> {
        Tape {
            backing: Box::new(self.backing),
            num_entries: self.num_entries,
        }
    }
}