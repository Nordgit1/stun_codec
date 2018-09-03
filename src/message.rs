use bytecodec::bytes::{BytesEncoder, CopyableBytesDecoder};
use bytecodec::combinator::{Collect, Length, Peekable, PreEncode, Repeat};
use bytecodec::fixnum::{U16beDecoder, U16beEncoder, U32beDecoder, U32beEncoder};
use bytecodec::{ByteCount, Decode, Encode, Eos, ErrorKind, Result, SizedEncode};
use std::marker::PhantomData;
use std::vec;

use attribute::{
    Attribute, LosslessAttribute, LosslessAttributeDecoder, LosslessAttributeEncoder, RawAttribute,
};
use constants::MAGIC_COOKIE;
use num::U12;
use {Method, TransactionId};

/// The class of a message.
#[allow(missing_docs)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MessageClass {
    Request,
    Indication,
    SuccessResponse,
    ErrorResponse,
}
impl MessageClass {
    fn from_u8(value: u8) -> Option<Self> {
        match value {
            0b00 => Some(MessageClass::Request),
            0b01 => Some(MessageClass::Indication),
            0b10 => Some(MessageClass::SuccessResponse),
            0b11 => Some(MessageClass::ErrorResponse),
            _ => None,
        }
    }
}

/// STUN message.
/// # NOTE: Binary Format of STUN Messages
///
/// > STUN messages are encoded in binary using network-oriented format
/// > (most significant byte or octet first, also commonly known as big-
/// > endian).  The transmission order is described in detail in Appendix B
/// > of [RFC 791].  Unless otherwise noted, numeric constants are
/// > in decimal (base 10).
/// >
/// > All STUN messages MUST start with a 20-byte header followed by zero
/// > or more Attributes.  The STUN header contains a STUN message type,
/// > magic cookie, transaction ID, and message length.
/// >
/// > ```text
/// >  0                   1                   2                   3
/// >  0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
/// > +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
/// > |0 0|     STUN Message Type     |         Message Length        |
/// > +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
/// > |                         Magic Cookie                          |
/// > +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
/// > |                                                               |
/// > |                     Transaction ID (96 bits)                  |
/// > |                                                               |
/// > +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
/// >
/// >             Figure 2: Format of STUN Message Header
/// > ```
/// >
/// > The most significant 2 bits of every STUN message MUST be zeroes.
/// > This can be used to differentiate STUN packets from other protocols
/// > when STUN is multiplexed with other protocols on the same port.
/// >
/// > The message type defines the message class (request, success
/// > response, failure response, or indication) and the message method
/// > (the primary function) of the STUN message.  Although there are four
/// > message classes, there are only two types of transactions in STUN:
/// > request/response transactions (which consist of a request message and
/// > a response message) and indication transactions (which consist of a
/// > single indication message).  Response classes are split into error
/// > and success responses to aid in quickly processing the STUN message.
/// >
/// > The message type field is decomposed further into the following structure:
/// >
/// > ```text
/// >  0                 1
/// >  2  3  4 5 6 7 8 9 0 1 2 3 4 5
/// > +--+--+-+-+-+-+-+-+-+-+-+-+-+-+
/// > |M |M |M|M|M|C|M|M|M|C|M|M|M|M|
/// > |11|10|9|8|7|1|6|5|4|0|3|2|1|0|
/// > +--+--+-+-+-+-+-+-+-+-+-+-+-+-+
/// >
/// > Figure 3: Format of STUN Message Type Field
/// > ```
/// >
/// > Here the bits in the message type field are shown as most significant
/// > (M11) through least significant (M0).  M11 through M0 represent a 12-
/// > bit encoding of the method.  C1 and C0 represent a 2-bit encoding of
/// > the class.  A class of 0b00 is a request, a class of 0b01 is an
/// > indication, a class of 0b10 is a success response, and a class of
/// > 0b11 is an error response.  This specification defines a single
/// > method, Binding.  The method and class are orthogonal, so that for
/// > each method, a request, success response, error response, and
/// > indication are possible for that method.  Extensions defining new
/// > methods MUST indicate which classes are permitted for that method.
/// >
/// > For example, a Binding request has class=0b00 (request) and
/// > method=0b000000000001 (Binding) and is encoded into the first 16 bits
/// > as 0x0001.  A Binding response has class=0b10 (success response) and
/// > method=0b000000000001, and is encoded into the first 16 bits as 0x0101.
/// >
/// > > Note: This unfortunate encoding is due to assignment of values in
/// > > [RFC 3489] that did not consider encoding Indications, Success, and
/// > > Errors using bit fields.
/// >
/// > The magic cookie field MUST contain the fixed value 0x2112A442 in
/// > network byte order.  In [RFC 3489], this field was part of
/// > the transaction ID; placing the magic cookie in this location allows
/// > a server to detect if the client will understand certain attributes
/// > that were added in this revised specification.  In addition, it aids
/// > in distinguishing STUN packets from packets of other protocols when
/// > STUN is multiplexed with those other protocols on the same port.
/// >
/// > The transaction ID is a 96-bit identifier, used to uniquely identify
/// > STUN transactions.  For request/response transactions, the
/// > transaction ID is chosen by the STUN client for the request and
/// > echoed by the server in the response.  For indications, it is chosen
/// > by the agent sending the indication.  It primarily serves to
/// > correlate requests with responses, though it also plays a small role
/// > in helping to prevent certain types of attacks.  The server also uses
/// > the transaction ID as a key to identify each transaction uniquely
/// > across all clients.  As such, the transaction ID MUST be uniformly
/// > and randomly chosen from the interval 0 .. 2**96-1, and SHOULD be
/// > cryptographically random.  Resends of the same request reuse the same
/// > transaction ID, but the client MUST choose a new transaction ID for
/// > new transactions unless the new request is bit-wise identical to the
/// > previous request and sent from the same transport address to the same
/// > IP address.  Success and error responses MUST carry the same
/// > transaction ID as their corresponding request.  When an agent is
/// > acting as a STUN server and STUN client on the same port, the
/// > transaction IDs in requests sent by the agent have no relationship to
/// > the transaction IDs in requests received by the agent.
/// >
/// > The message length MUST contain the size, in bytes, of the message
/// > not including the 20-byte STUN header.  Since all STUN attributes are
/// > padded to a multiple of 4 bytes, the last 2 bits of this field are
/// > always zero.  This provides another way to distinguish STUN packets
/// > from packets of other protocols.
/// >
/// > Following the STUN fixed portion of the header are zero or more
/// > attributes.  Each attribute is TLV (Type-Length-Value) encoded.  The
/// > details of the encoding, and of the attributes themselves are given
/// > in Section 15.
/// >
/// > [RFC 5389 -- 6. STUN Message Structure]
///
/// [RFC 5389 -- 6. STUN Message Structure]: https://tools.ietf.org/html/rfc5389#section-6
/// [RFC 791]: https://tools.ietf.org/html/rfc791
/// [RFC 3489]: https://tools.ietf.org/html/rfc3489
#[derive(Debug, Clone)]
pub struct Message<M, A> {
    class: MessageClass,
    method: M,
    transaction_id: TransactionId,
    attributes: Vec<LosslessAttribute<A>>,
}
impl<M: Method, A: Attribute> Message<M, A> {
    /// Makes a new `Message` instance.
    pub fn new(class: MessageClass, method: M, transaction_id: TransactionId) -> Self {
        Message {
            class,
            method,
            transaction_id,
            attributes: Vec::new(),
        }
    }

    /// Returns the class of the message.
    pub fn class(&self) -> MessageClass {
        self.class
    }

    /// Returns the method of the message.
    pub fn method(&self) -> &M {
        &self.method
    }

    /// Returns the transaction ID of the message.
    pub fn transaction_id(&self) -> &TransactionId {
        &self.transaction_id
    }

    /// Returns an iterator that iterates over the known attributes in the message.
    pub fn attributes(&self) -> impl Iterator<Item = &A> {
        self.attributes.iter().filter_map(|a| a.as_known())
    }

    /// Returns an iterator that iterates over the unknown attributes in the message.
    ///
    /// Note that it is the responsibility of users to check
    /// whether the unknown attributes contains comprehension-required ones.
    pub fn unknown_attributes(&self) -> impl Iterator<Item = &RawAttribute> {
        self.attributes.iter().filter_map(|a| a.as_unknown())
    }

    /// Adds the given attribute to the tail of the attributes in the message.
    pub fn push_attribute(&mut self, attribute: A) {
        self.attributes.push(LosslessAttribute::new(attribute));
    }
}

#[derive(Debug, Default)]
struct MessageHeaderDecoder {
    message_type: U16beDecoder,
    message_len: U16beDecoder,
    magic_cookie: U32beDecoder,
    transaction_id: CopyableBytesDecoder<[u8; 12]>,
}
impl Decode for MessageHeaderDecoder {
    type Item = (Type, u16, TransactionId);

    fn decode(&mut self, buf: &[u8], eos: Eos) -> Result<usize> {
        let mut offset = 0;
        bytecodec_try_decode!(self.message_type, offset, buf, eos);
        bytecodec_try_decode!(self.message_len, offset, buf, eos);
        bytecodec_try_decode!(self.magic_cookie, offset, buf, eos);
        bytecodec_try_decode!(self.transaction_id, offset, buf, eos);
        Ok(offset)
    }

    fn finish_decoding(&mut self) -> Result<Self::Item> {
        let message_type = track!(self.message_type.finish_decoding())?;
        let message_type = track!(Type::from_u16(message_type))?;
        let message_len = track!(self.message_len.finish_decoding())?;
        let magic_cookie = track!(self.magic_cookie.finish_decoding())?;
        let transaction_id = TransactionId::new(track!(self.transaction_id.finish_decoding())?);
        track_assert_eq!(magic_cookie, MAGIC_COOKIE, ErrorKind::InvalidInput);
        Ok((message_type, message_len, transaction_id))
    }

    fn requiring_bytes(&self) -> ByteCount {
        self.message_type
            .requiring_bytes()
            .add_for_decoding(self.message_len.requiring_bytes())
            .add_for_decoding(self.magic_cookie.requiring_bytes())
            .add_for_decoding(self.transaction_id.requiring_bytes())
    }

    fn is_idle(&self) -> bool {
        self.transaction_id.is_idle()
    }
}

/// [`Message`] decoder.
///
/// [`Message`]: ./struct.Message.html
#[derive(Debug)]
pub struct MessageDecoder<M: Method, A: Attribute> {
    header: Peekable<MessageHeaderDecoder>,
    attributes: Length<Collect<LosslessAttributeDecoder<A>, Vec<LosslessAttribute<A>>>>,
    _phantom: PhantomData<M>,
}
impl<M: Method, A: Attribute> MessageDecoder<M, A> {
    /// Makes a new `MessageDecoder` instance.
    pub fn new() -> Self {
        Self::default()
    }
}
impl<M: Method, A: Attribute> Default for MessageDecoder<M, A> {
    fn default() -> Self {
        MessageDecoder {
            header: Default::default(),
            attributes: Default::default(),
            _phantom: Default::default(),
        }
    }
}
impl<M: Method, A: Attribute> Decode for MessageDecoder<M, A> {
    type Item = Message<M, A>;

    fn decode(&mut self, buf: &[u8], eos: Eos) -> Result<usize> {
        let mut offset = 0;
        if !self.header.is_idle() {
            bytecodec_try_decode!(self.header, offset, buf, eos);

            let message_len = self.header.peek().expect("never fails").1;
            track!(self.attributes.set_expected_bytes(u64::from(message_len)))?;
        }
        bytecodec_try_decode!(self.attributes, offset, buf, eos);
        Ok(offset)
    }

    fn finish_decoding(&mut self) -> Result<Self::Item> {
        let (message_type, _, transaction_id) = track!(self.header.finish_decoding())?;
        let attributes = track!(self.attributes.finish_decoding())?;
        let method = track_assert_some!(
            M::from_u12(message_type.method),
            ErrorKind::InvalidInput,
            "Unknown STUN method: {}",
            message_type.method
        );
        let mut message = Message {
            class: message_type.class,
            method,
            transaction_id,
            attributes,
        };

        let attributes_len = message.attributes.len();
        for i in 0..attributes_len {
            unsafe {
                message.attributes.set_len(i);
                let message_mut = &mut *(&mut message as *mut Message<M, A>);
                let attr = message_mut.attributes.get_unchecked_mut(i);
                if let Err(e) = track!(attr.after_decode(&message)) {
                    message.attributes.set_len(attributes_len);
                    return Err(e);
                }
            }
        }
        unsafe {
            message.attributes.set_len(attributes_len);
        }
        Ok(message)
    }

    fn requiring_bytes(&self) -> ByteCount {
        self.header
            .requiring_bytes()
            .add_for_decoding(self.attributes.requiring_bytes())
    }

    fn is_idle(&self) -> bool {
        self.header.is_idle() && self.attributes.is_idle()
    }
}

/// [`Message`] encoder.
///
/// [`Message`]: ./struct.Message.html
pub struct MessageEncoder<M: Method, A: Attribute> {
    message_type: U16beEncoder,
    message_len: U16beEncoder,
    magic_cookie: U32beEncoder,
    transaction_id: BytesEncoder<TransactionId>,
    attributes: PreEncode<Repeat<LosslessAttributeEncoder<A>, vec::IntoIter<LosslessAttribute<A>>>>,
    _phantom: PhantomData<M>,
}
impl<M: Method, A: Attribute> MessageEncoder<M, A> {
    /// Makes a new `MessageEncoder` instance.
    pub fn new() -> Self {
        Self::default()
    }
}
impl<M: Method, A: Attribute> Default for MessageEncoder<M, A> {
    fn default() -> Self {
        MessageEncoder {
            message_type: Default::default(),
            message_len: Default::default(),
            magic_cookie: Default::default(),
            transaction_id: Default::default(),
            attributes: Default::default(),
            _phantom: Default::default(),
        }
    }
}
impl<M: Method, A: Attribute> Encode for MessageEncoder<M, A> {
    type Item = Message<M, A>;

    fn encode(&mut self, buf: &mut [u8], eos: Eos) -> Result<usize> {
        let mut offset = 0;
        bytecodec_try_encode!(self.message_type, offset, buf, eos);
        bytecodec_try_encode!(self.message_len, offset, buf, eos);
        bytecodec_try_encode!(self.magic_cookie, offset, buf, eos);
        bytecodec_try_encode!(self.transaction_id, offset, buf, eos);
        bytecodec_try_encode!(self.attributes, offset, buf, eos);
        Ok(offset)
    }

    fn start_encoding(&mut self, mut item: Self::Item) -> Result<()> {
        let attributes_len = item.attributes.len();
        for i in 0..attributes_len {
            unsafe {
                item.attributes.set_len(i);
                let item_mut = &mut *(&mut item as *mut Message<M, A>);
                let attr = item_mut.attributes.get_unchecked_mut(i);
                if let Err(e) = track!(attr.before_encode(&item)) {
                    item.attributes.set_len(attributes_len);
                    return Err(e);
                }
            }
        }
        unsafe {
            item.attributes.set_len(attributes_len);
        }

        let message_type = Type {
            class: item.class,
            method: item.method.as_u12(),
        };
        track!(self.message_type.start_encoding(message_type.as_u16()))?;
        track!(self.magic_cookie.start_encoding(MAGIC_COOKIE))?;
        track!(self.transaction_id.start_encoding(item.transaction_id))?;
        track!(self.attributes.start_encoding(item.attributes.into_iter()))?;

        let message_len = self.attributes.exact_requiring_bytes();
        track_assert!(
            message_len < 0x10000,
            ErrorKind::InvalidInput,
            "Too large message length: actual={}, limit=0xFFFF",
            message_len
        );
        track!(self.message_len.start_encoding(message_len as u16))?;
        Ok(())
    }

    fn requiring_bytes(&self) -> ByteCount {
        ByteCount::Finite(self.exact_requiring_bytes())
    }

    fn is_idle(&self) -> bool {
        self.transaction_id.is_idle() && self.attributes.is_idle()
    }
}
impl<M: Method, A: Attribute> SizedEncode for MessageEncoder<M, A> {
    fn exact_requiring_bytes(&self) -> u64 {
        self.message_type.exact_requiring_bytes()
            + self.message_len.exact_requiring_bytes()
            + self.magic_cookie.exact_requiring_bytes()
            + self.transaction_id.exact_requiring_bytes()
            + self.attributes.exact_requiring_bytes()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Type {
    pub class: MessageClass,
    pub method: U12,
}
impl Type {
    pub fn as_u16(&self) -> u16 {
        let class = self.class as u16;
        let method = self.method.as_u16();
        ((method & 0b0000_0000_1111) << 0)
            | ((class & 0b01) << 4)
            | ((method & 0b0000_0111_0000) << 5)
            | ((class & 0b10) << 7)
            | ((method & 0b1111_1000_0000) << 9)
    }

    pub fn from_u16(value: u16) -> Result<Self> {
        track_assert!(
            value >> 14 == 0,
            ErrorKind::InvalidInput,
            "First two-bits of STUN message must be 0"
        );
        let class = ((value >> 4) & 0b01) | ((value >> 7) & 0b10);
        let class = MessageClass::from_u8(class as u8).unwrap();
        let method = (value & 0b0000_0000_1111)
            | ((value >> 1) & 0b0000_0111_0000)
            | ((value >> 2) & 0b1111_1000_0000);
        let method = U12::from_u16(method).expect("never fails");
        Ok(Type {
            class: class,
            method: method,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_class_from_u8_works() {
        assert_eq!(MessageClass::from_u8(0), Some(MessageClass::Request));
        assert_eq!(MessageClass::from_u8(9), None);
    }
}
