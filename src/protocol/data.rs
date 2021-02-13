use bytes::Bytes;

/// Binary message data
#[derive(Debug, Clone)]
pub struct MessageData(MessageDataImpl);

/// opaque inner type to allow modifying the implementation in the future
#[derive(Debug, Clone)]
enum MessageDataImpl {
    Shared(Bytes),
    Unique(Vec<u8>),
}

impl MessageData {
    pub fn len(&self) -> usize {
        self.as_ref().len()
    }

    fn make_unique(&mut self) {
        if let MessageDataImpl::Shared(data) = &self.0 {
            self.0 = MessageDataImpl::Unique(Vec::from(data.as_ref()));
        }
    }
}

impl PartialEq for MessageData {
    fn eq(&self, other: &MessageData) -> bool {
        self.as_ref().eq(other.as_ref())
    }
}

impl Eq for MessageData {}

impl From<MessageData> for Vec<u8> {
    fn from(data: MessageData) -> Vec<u8> {
        match data.0 {
            MessageDataImpl::Shared(data) => {
                let mut bytes = Vec::with_capacity(data.len());
                bytes.copy_from_slice(data.as_ref());
                bytes
            }
            MessageDataImpl::Unique(data) => data,
        }
    }
}

impl From<MessageData> for Bytes {
    fn from(data: MessageData) -> Bytes {
        match data.0 {
            MessageDataImpl::Shared(data) => data,
            MessageDataImpl::Unique(data) => data.into(),
        }
    }
}

impl AsRef<[u8]> for MessageData {
    fn as_ref(&self) -> &[u8] {
        match &self.0 {
            MessageDataImpl::Shared(data) => data.as_ref(),
            MessageDataImpl::Unique(data) => data.as_ref(),
        }
    }
}

impl AsMut<[u8]> for MessageData {
    fn as_mut(&mut self) -> &mut [u8] {
        self.make_unique();
        match &mut self.0 {
            MessageDataImpl::Unique(data) => data.as_mut_slice(),
            MessageDataImpl::Shared(_) => unreachable!("Data has just been made unique"),
        }
    }
}

/// String message data
#[derive(Debug, Clone)]
pub struct MessageStringData(MessageStringDataImpl);

/// opaque inner type to allow modifying the implementation in the future
#[derive(Debug, Clone)]
enum MessageStringDataImpl {
    Static(&'static str),
    Unique(String),
}

impl PartialEq for MessageStringData {
    fn eq(&self, other: &MessageStringData) -> bool {
        self.as_ref().eq(other.as_ref())
    }
}

impl Eq for MessageStringData {}

impl From<MessageStringData> for String {
    fn from(data: MessageStringData) -> String {
        match data.0 {
            MessageStringDataImpl::Static(data) => data.into(),
            MessageStringDataImpl::Unique(data) => data,
        }
    }
}

impl From<MessageStringData> for MessageData {
    fn from(data: MessageStringData) -> MessageData {
        match data.0 {
            MessageStringDataImpl::Static(data) => MessageData::from(data.as_bytes()),
            MessageStringDataImpl::Unique(data) => MessageData::from(data.into_bytes()),
        }
    }
}

impl AsRef<str> for MessageStringData {
    fn as_ref(&self) -> &str {
        match &self.0 {
            MessageStringDataImpl::Static(data) => *data,
            MessageStringDataImpl::Unique(data) => data.as_ref(),
        }
    }
}

impl From<String> for MessageStringData {
    fn from(string: String) -> MessageStringData {
        MessageStringData(MessageStringDataImpl::Unique(string))
    }
}

impl From<&'static str> for MessageStringData {
    fn from(string: &'static str) -> MessageStringData {
        MessageStringData(MessageStringDataImpl::Static(string))
    }
}

impl From<Vec<u8>> for MessageData {
    fn from(data: Vec<u8>) -> MessageData {
        MessageData(MessageDataImpl::Unique(data))
    }
}

impl From<&'static [u8]> for MessageData {
    fn from(data: &'static [u8]) -> MessageData {
        MessageData(MessageDataImpl::Shared(Bytes::from_static(data)))
    }
}

impl From<Bytes> for MessageData {
    fn from(data: Bytes) -> MessageData {
        MessageData(MessageDataImpl::Shared(data))
    }
}
