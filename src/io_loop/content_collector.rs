use crate::{AmqpProperties, Delivery, ErrorKind, Result};
use amq_protocol::frame::AMQPContentHeader;
use amq_protocol::protocol::basic::Deliver;

pub(super) struct ContentCollector {
    kind: Option<Kind>,
}

pub(super) enum CollectorResult {
    Delivery((String, Delivery)),
}

impl ContentCollector {
    pub(super) fn new() -> ContentCollector {
        ContentCollector { kind: None }
    }

    pub(super) fn collect_deliver(&mut self, deliver: Deliver) -> Result<()> {
        match self.kind.take() {
            None => {
                self.kind = Some(Kind::Delivery(State::Start(deliver)));
                Ok(())
            }
            Some(_) => Err(ErrorKind::FrameUnexpected)?,
        }
    }

    pub(super) fn collect_header(
        &mut self,
        header: AMQPContentHeader,
    ) -> Result<Option<CollectorResult>> {
        match self.kind.take() {
            Some(Kind::Delivery(state)) => match state.collect_header(header)? {
                Content::Done((tag, delivery)) => {
                    self.kind = None;
                    Ok(Some(CollectorResult::Delivery((tag, delivery))))
                }
                Content::NeedMore(state) => {
                    self.kind = Some(Kind::Delivery(state));
                    Ok(None)
                }
            },
            None => Err(ErrorKind::FrameUnexpected)?,
        }
    }

    pub(super) fn collect_body(&mut self, body: Vec<u8>) -> Result<Option<CollectorResult>> {
        match self.kind.take() {
            Some(Kind::Delivery(state)) => match state.collect_body(body)? {
                Content::Done((tag, delivery)) => {
                    self.kind = None;
                    Ok(Some(CollectorResult::Delivery((tag, delivery))))
                }
                Content::NeedMore(state) => {
                    self.kind = Some(Kind::Delivery(state));
                    Ok(None)
                }
            },
            None => Err(ErrorKind::FrameUnexpected)?,
        }
    }
}

enum Kind {
    Delivery(State<Delivery>),
}

trait ContentType {
    type Start;
    type Finish;

    fn new(start: Self::Start, buf: Vec<u8>, properties: AmqpProperties) -> Self::Finish;
}

impl ContentType for Delivery {
    type Start = Deliver;
    type Finish = (String, Delivery);

    fn new(start: Self::Start, buf: Vec<u8>, properties: AmqpProperties) -> Self::Finish {
        Delivery::new(start, buf, properties)
    }
}

enum Content<T: ContentType> {
    Done(T::Finish),
    NeedMore(State<T>),
}

enum State<T: ContentType> {
    Start(T::Start),
    Body(T::Start, AMQPContentHeader, Vec<u8>),
}

impl<T: ContentType> State<T> {
    fn collect_header(self, header: AMQPContentHeader) -> Result<Content<T>> {
        match self {
            State::Start(start) => {
                if header.body_size == 0 {
                    Ok(Content::Done(T::new(
                        start,
                        Vec::new(),
                        header.properties,
                    )))
                } else {
                    let buf = Vec::with_capacity(header.body_size as usize);
                    Ok(Content::NeedMore(State::Body(start, header, buf)))
                }
            }
            State::Body(_, _, _) => Err(ErrorKind::FrameUnexpected)?,
        }
    }

    fn collect_body(self, mut body: Vec<u8>) -> Result<Content<T>> {
        match self {
            State::Body(start, header, mut buf) => {
                let body_size = header.body_size as usize;
                buf.append(&mut body);
                if buf.len() == body_size {
                    Ok(Content::Done(T::new(start, buf, header.properties)))
                } else if buf.len() < body_size {
                    Ok(Content::NeedMore(State::Body(start, header, buf)))
                } else {
                    Err(ErrorKind::FrameUnexpected)?
                }
            }
            State::Start(_) => Err(ErrorKind::FrameUnexpected)?,
        }
    }
}