use std::mem::{align_of, size_of};
use std::slice;

use ndarray::{ArrayViewD, ArrayViewMutD, IxDyn, ShapeBuilder};

use crate::client::SharedObject;
use crate::error::{MStoreError, Result};

/// Primitive element types that may be viewed directly in an mstore mapping.
///
/// # Safety
/// Implementors must be plain-old-data with no padding-sensitive invariants,
/// references, drop glue, or invalid bit patterns for bytes written by peers.
pub unsafe trait ShmElement: Copy + 'static {
    const KIND: char;

    fn numpy_dtype() -> String {
        let itemsize = size_of::<Self>();
        let endian = if itemsize == 1 {
            '|'
        } else if cfg!(target_endian = "little") {
            '<'
        } else {
            '>'
        };
        format!("{endian}{}{itemsize}", Self::KIND)
    }
}

macro_rules! impl_element {
    ($ty:ty, $kind:literal) => {
        unsafe impl ShmElement for $ty {
            const KIND: char = $kind;
        }
    };
}

impl_element!(u8, 'u');
impl_element!(i8, 'i');
impl_element!(u16, 'u');
impl_element!(i16, 'i');
impl_element!(u32, 'u');
impl_element!(i32, 'i');
impl_element!(u64, 'u');
impl_element!(i64, 'i');
impl_element!(f32, 'f');
impl_element!(f64, 'f');

fn validate_dtype<T: ShmElement>(dtype: &str) -> Result<()> {
    if dtype.len() < 3 {
        return Err(MStoreError::Array(format!(
            "unsupported NumPy dtype metadata: {dtype:?}"
        )));
    }

    let mut chars = dtype.chars();
    let endian = chars.next().unwrap();
    let kind = chars.next().unwrap();
    let itemsize: usize = chars
        .as_str()
        .parse()
        .map_err(|_| MStoreError::Array(format!("invalid NumPy dtype: {dtype:?}")))?;

    if kind != T::KIND || itemsize != size_of::<T>() {
        return Err(MStoreError::Array(format!(
            "dtype mismatch: object is {dtype:?}, requested {}",
            T::numpy_dtype()
        )));
    }

    if itemsize > 1 {
        let native = if cfg!(target_endian = "little") {
            '<'
        } else {
            '>'
        };
        if !matches!(endian, '=' | '|') && endian != native {
            return Err(MStoreError::Array(format!(
                "non-native-endian dtype {dtype:?} requires explicit conversion"
            )));
        }
    }
    Ok(())
}

impl SharedObject {
    /// Create a closure-scoped, zero-copy ndarray view.
    ///
    /// # Safety
    /// The caller must prevent unsynchronized cross-process mutation of the mapped
    /// bytes for the duration of the closure.
    pub unsafe fn with_array<T: ShmElement, R>(
        &self,
        f: impl FnOnce(ArrayViewD<'_, T>) -> R,
    ) -> Result<R> {
        let shape = self
            .shape()
            .ok_or_else(|| MStoreError::Array("object has no shape metadata".into()))?
            .to_vec();
        let dtype = self
            .dtype()
            .ok_or_else(|| MStoreError::Array("object has no dtype metadata".into()))?;
        validate_dtype::<T>(dtype)?;

        let expected = shape
            .iter()
            .try_fold(1usize, |acc, &x| acc.checked_mul(x))
            .and_then(|count| count.checked_mul(size_of::<T>()))
            .ok_or_else(|| MStoreError::Array("array size overflow".into()))?;
        if expected != self.size() as usize {
            return Err(MStoreError::Array(format!(
                "metadata size mismatch: expected {expected} bytes, object has {}",
                self.size()
            )));
        }

        let is_fortran = self.order() == "F";
        // SAFETY: forwarded from the caller of with_array.
        unsafe {
            self.with_bytes(|bytes| -> Result<R> {
                if bytes.as_ptr().align_offset(align_of::<T>()) != 0 {
                    return Err(MStoreError::Array(
                        "mapping is not correctly aligned".into(),
                    ));
                }
                // SAFETY: ShmElement is restricted to POD primitives; alignment,
                // byte length and dtype were checked above, and the view cannot escape.
                let elements =
                    slice::from_raw_parts(bytes.as_ptr().cast::<T>(), bytes.len() / size_of::<T>());
                let builder = IxDyn(&shape).set_f(is_fortran);
                let view = ArrayViewD::from_shape(builder, elements)
                    .map_err(|e| MStoreError::Array(format!("invalid array shape: {e}")))?;
                Ok(f(view))
            })
        }
    }

    /// Create a closure-scoped, zero-copy mutable ndarray view.
    ///
    /// # Safety
    /// The caller must hold exclusive access to the affected shared-memory bytes
    /// across all processes for the duration of the closure.
    pub unsafe fn with_array_mut<T: ShmElement, R>(
        &self,
        f: impl FnOnce(ArrayViewMutD<'_, T>) -> R,
    ) -> Result<R> {
        let shape = self
            .shape()
            .ok_or_else(|| MStoreError::Array("object has no shape metadata".into()))?
            .to_vec();
        let dtype = self
            .dtype()
            .ok_or_else(|| MStoreError::Array("object has no dtype metadata".into()))?;
        validate_dtype::<T>(dtype)?;

        let expected = shape
            .iter()
            .try_fold(1usize, |acc, &x| acc.checked_mul(x))
            .and_then(|count| count.checked_mul(size_of::<T>()))
            .ok_or_else(|| MStoreError::Array("array size overflow".into()))?;
        if expected != self.size() as usize {
            return Err(MStoreError::Array(format!(
                "metadata size mismatch: expected {expected} bytes, object has {}",
                self.size()
            )));
        }

        let is_fortran = self.order() == "F";
        // SAFETY: forwarded from the caller of with_array_mut.
        let nested = unsafe {
            self.with_bytes_mut(|bytes| -> Result<R> {
                if bytes.as_ptr().align_offset(align_of::<T>()) != 0 {
                    return Err(MStoreError::Array(
                        "mapping is not correctly aligned".into(),
                    ));
                }
                // SAFETY: see with_array; the MappingState mutex also serializes
                // mutable access through this process-local cached mapping.
                let elements = slice::from_raw_parts_mut(
                    bytes.as_mut_ptr().cast::<T>(),
                    bytes.len() / size_of::<T>(),
                );
                let builder = IxDyn(&shape).set_f(is_fortran);
                let view = ArrayViewMutD::from_shape(builder, elements)
                    .map_err(|e| MStoreError::Array(format!("invalid array shape: {e}")))?;
                Ok(f(view))
            })
        }?;
        nested
    }
}
