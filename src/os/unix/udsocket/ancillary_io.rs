use super::cmsg::*;
use std::{
    fmt::Arguments,
    io::{self, prelude::*, IoSlice, IoSliceMut},
    ops::{Add, AddAssign},
};

/// The successful result of an ancillary-enabled read.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct ReadAncillarySuccess {
    /// How many bytes were read to the main buffer.
    pub main: usize,
    /// How many bytes were read to the ancillary buffer.
    pub ancillary: usize,
}
impl Add for ReadAncillarySuccess {
    type Output = Self;
    #[inline(always)]
    fn add(self, rhs: Self) -> Self::Output {
        Self {
            main: self.main + rhs.main,
            ancillary: self.ancillary + rhs.ancillary,
        }
    }
}
impl AddAssign for ReadAncillarySuccess {
    #[inline(always)]
    fn add_assign(&mut self, rhs: Self) {
        *self = *self + rhs;
    }
}

/// An extension of [`Read`] that enables operations involving ancillary data.
///
/// The generic parameter on the trait allows for trait objects to be constructed. Simply substitute [`DynCmsgMut`] or
/// [`DynCmsgMutStatic`] for `AB` to obtain an object-safe `ReadAncillary`.
pub trait ReadAncillary<AB: CmsgMut + ?Sized>: Read {
    /// Analogous to [`Read::read()`], but also reads control messages into the given ancillary buffer.
    ///
    /// The return value contains both the amount of main-band data read into the given regular buffer and the number of
    /// bytes read into the ancillary buffer.
    fn read_ancillary(&mut self, buf: &mut [u8], abuf: &mut AB) -> io::Result<ReadAncillarySuccess>;

    /// Same as [`read_ancillary`](ReadAncillary::read_ancillary), but performs a
    /// [scatter read](https://en.wikipedia.org/wiki/Vectored_I%2FO) instead.
    fn read_ancillary_vectored(
        &mut self,
        bufs: &mut [IoSliceMut<'_>],
        abuf: &mut AB,
    ) -> io::Result<ReadAncillarySuccess> {
        let buf = bufs
            .iter_mut()
            .find(|b| !b.is_empty())
            .map_or(&mut [][..], |b| &mut **b);
        self.read_ancillary(buf, abuf)
    }
}

pub(super) fn read_in_terms_of_vectored<AB: CmsgMut + ?Sized>(
    slf: &mut impl ReadAncillary<AB>,
    buf: &mut [u8],
    abuf: &mut AB,
) -> io::Result<ReadAncillarySuccess> {
    slf.read_ancillary_vectored(&mut [IoSliceMut::new(buf)], abuf)
}

#[cfg(debug_assertions)]
fn _assert_read_ancillary_object_safe<'j: 'm + 'c, 'm, 'c, T: ReadAncillary<DynCmsgMut<'m, 'c>> + 'j>(
    x: &mut T,
) -> &mut (dyn ReadAncillaryExt<DynCmsgMut<'m, 'c>> + 'j) {
    x as _
}

impl<AB: CmsgMut + ?Sized, T: ReadAncillary<AB> + ?Sized> ReadAncillary<AB> for &mut T {
    forward_trait_method!(
        fn read_ancillary(&mut self, buf: &mut [u8], abuf: &mut AB)
            -> io::Result<ReadAncillarySuccess>
    );
    forward_trait_method!(
        fn read_ancillary_vectored(
            &mut self,
            bufs: &mut [IoSliceMut<'_>],
            abuf: &mut AB,
        ) -> io::Result<ReadAncillarySuccess>
    );
}
impl<AB: CmsgMut + ?Sized, T: ReadAncillary<AB> + ?Sized> ReadAncillary<AB> for Box<T> {
    forward_trait_method!(
        fn read_ancillary(&mut self, buf: &mut [u8], abuf: &mut AB)
            -> io::Result<ReadAncillarySuccess>
    );
    forward_trait_method!(
        fn read_ancillary_vectored(
            &mut self,
            bufs: &mut [IoSliceMut<'_>],
            abuf: &mut AB,
        ) -> io::Result<ReadAncillarySuccess>
    );
}

/// Methods derived from the interface of [`ReadAncillary`].
///
/// See the documentation on `ReadAncillary` for notes on why a type parameter is present.
pub trait ReadAncillaryExt<AB: CmsgMut + ?Sized>: ReadAncillary<AB> {
    /// Analogous to [`Read::read_to_end()`], but also reads ancillary data into the given ancillary buffer, growing it
    /// with the regular data buffer.
    ///
    /// **Read-to-end semantics apply to both main and ancillary data**, unlike with [`read_to_end_with_ancillary()`],
    /// which only grows the main data buffer and reads ancillary data exactly the same way as a regular
    /// [`read_ancillary`](ReadAncillary::read_ancillary) operation would.
    ///
    /// Note that using a buffer type that doesn't support resizing, such as [`CmsgMutBuf`], will produce identical
    /// behavior to [`read_to_end_with_ancillary()`].
    ///
    /// [`read_to_end_with_ancillary()`]: ReadAncillaryExt::read_to_end_with_ancillary
    fn read_ancillary_to_end(&mut self, buf: &mut Vec<u8>, abuf: &mut AB) -> io::Result<ReadAncillarySuccess> {
        let mut partappl = ReadAncillaryPartAppl {
            slf: self,
            abuf,
            ret: Default::default(),
            reserve: true,
        };
        partappl.read_to_end(buf)?;
        Ok(partappl.ret)
    }

    /// Analogous to [`Read::read_to_end()`], but also reads ancillary data into the given ancillary buffer.
    ///
    /// **Read-to-end semantics apply only to the main data**, unlike with
    /// [`read_ancillary_to_end()`](ReadAncillaryExt::read_ancillary_to_end), which grows both buffers adaptively and
    /// thus requires both of them to be passed with ownership.
    fn read_to_end_with_ancillary(&mut self, buf: &mut Vec<u8>, abuf: &mut AB) -> io::Result<ReadAncillarySuccess> {
        let mut partappl = ReadAncillaryPartAppl {
            slf: self,
            abuf,
            ret: Default::default(),
            reserve: false,
        };
        partappl.read_to_end(buf)?;
        Ok(partappl.ret)
    }

    /// Analogous to [`Read::read_exact`], but also reads ancillary data into the given buffer.
    fn read_exact_with_ancillary(&mut self, buf: &mut [u8], abuf: &mut AB) -> io::Result<ReadAncillarySuccess> {
        let mut partappl = ReadAncillaryPartAppl {
            slf: self,
            abuf,
            ret: Default::default(),
            reserve: false,
        };
        partappl.read_exact(buf)?;
        Ok(partappl.ret)
    }
}
impl<AB: CmsgMut + ?Sized, T: ReadAncillary<AB> + ?Sized> ReadAncillaryExt<AB> for T {}

/// Partial application of `read_ancillary`, storing the ancillary buffer and providing a `Read` interface.
///
/// Used here to piggyback off of the standard library's `read_to_end` implementation.
struct ReadAncillaryPartAppl<'s, 'a, RA: ?Sized, AB: ?Sized> {
    slf: &'s mut RA,
    abuf: &'a mut AB,
    /// An accumulator for the return value.
    ret: ReadAncillarySuccess,
    /// Whether to reserve together with the main-band buffer.
    reserve: bool,
}
impl<RA: ReadAncillary<AB> + ?Sized, AB: CmsgMut + ?Sized> Read for ReadAncillaryPartAppl<'_, '_, RA, AB> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.reserve {
            // The `exact` variant is used here because the `read_to_end` implementation from the standard library
            // has its own clever decision-making for the buffer size and we don't want the ancillary buffer to perform
            // guesswork which the main buffer has already performed. The result we ignore because not resizing is
            // intended behavior for when it's impossible.
            let _ = self.abuf.reserve_up_to_exact(buf.len());
        }
        let sc = self.slf.read_ancillary(buf, self.abuf)?;
        self.ret += sc;
        Ok(sc.main)
    }
}

/// An extension of [`Write`] that enables operations involving ancillary data.
pub trait WriteAncillary: Write {
    /// Analogous to [`Write::write()`], but also sends control messages from the given ancillary buffer
    ///
    /// The return value only the amount of main-band data sent from the given regular buffer – the entirety of the
    /// given `abuf` is always sent in full.
    fn write_ancillary(&mut self, buf: &[u8], abuf: CmsgRef<'_, '_>) -> io::Result<usize>;

    /// Same as [`write_ancillary`](WriteAncillary::write_ancillary), but performs a
    /// [gather write](https://en.wikipedia.org/wiki/Vectored_I%2FO) instead.
    fn write_ancillary_vectored(&mut self, bufs: &[IoSlice<'_>], abuf: CmsgRef<'_, '_>) -> io::Result<usize> {
        let buf = bufs.iter().find(|b| !b.is_empty()).map_or(&[][..], |b| &**b);
        self.write_ancillary(buf, abuf)
    }
}

impl<T: WriteAncillary + ?Sized> WriteAncillary for &mut T {
    forward_trait_method!(
        fn write_ancillary(&mut self, buf: &[u8], abuf: CmsgRef<'_, '_>)
            -> io::Result<usize>
    );
    forward_trait_method!(
        fn write_ancillary_vectored(
            &mut self,
            bufs: &[IoSlice<'_>],
            abuf: CmsgRef<'_, '_>,
        ) -> io::Result<usize>
    );
}
impl<T: WriteAncillary + ?Sized> WriteAncillary for Box<T> {
    forward_trait_method!(
        fn write_ancillary(&mut self, buf: &[u8], abuf: CmsgRef<'_, '_>)
            -> io::Result<usize>
    );
    forward_trait_method!(
        fn write_ancillary_vectored(
            &mut self,
            bufs: &[IoSlice<'_>],
            abuf: CmsgRef<'_, '_>,
        ) -> io::Result<usize>
    );
}
pub(super) fn write_in_terms_of_vectored(
    slf: &mut impl WriteAncillary,
    buf: &[u8],
    abuf: CmsgRef<'_, '_>,
) -> io::Result<usize> {
    slf.write_ancillary_vectored(&[IoSlice::new(buf)], abuf)
}

#[cfg(debug_assertions)]
fn _assert_write_ancillary_object_safe<'a, T: WriteAncillary + 'a>(x: &mut T) -> &mut (dyn WriteAncillaryExt + 'a) {
    x
}

/// Methods derived from the interface of [`WriteAncillary`].
pub trait WriteAncillaryExt: WriteAncillary {
    /// Analogous to [`write_all`](Write::write_all), but also writes ancillary data.
    fn write_all_ancillary(&mut self, buf: &[u8], abuf: CmsgRef<'_, '_>) -> io::Result<()> {
        let mut partappl = WriteAncillaryPartAppl { slf: self, abuf };
        partappl.write_all(buf)
    }

    /// Analogous to [`write_fmt`](Write::write_fmt), but also writes ancillary data.
    fn write_fmt_ancillary(&mut self, fmt: Arguments<'_>, abuf: CmsgRef<'_, '_>) -> io::Result<()> {
        let mut partappl = WriteAncillaryPartAppl { slf: self, abuf };
        partappl.write_fmt(fmt)
    }
}
impl<T: WriteAncillary + ?Sized> WriteAncillaryExt for T {}

/// Like `ReadAncillaryPartAppl`, but for `WriteAncillary` instead.
struct WriteAncillaryPartAppl<'slf, 'b, 'c, WA: ?Sized> {
    slf: &'slf mut WA,
    abuf: CmsgRef<'b, 'c>,
}
impl<WA: WriteAncillary + ?Sized> Write for WriteAncillaryPartAppl<'_, '_, '_, WA> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let bytes_written = if !self.abuf.inner().is_empty() {
            let bw = self.slf.write_ancillary(buf, self.abuf)?;
            self.abuf.consume_bytes(self.abuf.inner().len());
            bw
        } else {
            self.slf.write(buf)?
        };
        Ok(bytes_written)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.slf.flush()
    }
}
