use super::{
    ChannelMessage, ConnectionBlockedNotification, ConsumerMessage, IoLoopCommand, IoLoopMessage,
    IoLoopRpc,
};
use crate::serialize::{IntoAmqpClass, OutputBuffer, TryFromAmqpClass};
use crate::{AmqpProperties, Error, ErrorKind, Result};
use amq_protocol::protocol::basic::AMQPMethod as AmqpBasic;
use amq_protocol::protocol::basic::Consume;
use crossbeam_channel::Receiver as CrossbeamReceiver;
use log::error;
use mio_extras::channel::SyncSender as MioSyncSender;
use std::ops::{Deref, DerefMut};

pub(super) struct IoLoopHandle {
    pub(super) channel_id: u16,
    pub(super) buf: OutputBuffer,
    tx: MioSyncSender<IoLoopMessage>,
    rx: CrossbeamReceiver<Result<ChannelMessage>>,
}

impl IoLoopHandle {
    pub(super) fn new(
        channel_id: u16,
        tx: MioSyncSender<IoLoopMessage>,
        rx: CrossbeamReceiver<Result<ChannelMessage>>,
    ) -> IoLoopHandle {
        IoLoopHandle {
            channel_id,
            buf: OutputBuffer::empty(),
            tx,
            rx,
        }
    }

    pub(super) fn make_buf<M: IntoAmqpClass>(&mut self, method: M) -> Result<OutputBuffer> {
        debug_assert!(self.buf.is_empty());
        self.buf.push_method(self.channel_id, method)?;
        Ok(self.buf.drain_into_new_buf())
    }

    pub(super) fn send_command(&mut self, command: IoLoopCommand) -> Result<()> {
        self.send(IoLoopMessage::Command(command))
    }

    pub(super) fn consume(
        &mut self,
        consume: Consume,
    ) -> Result<(String, CrossbeamReceiver<ConsumerMessage>)> {
        let buf = self.make_buf(AmqpBasic::Consume(consume))?;
        self.send(IoLoopMessage::Rpc(IoLoopRpc::Send(buf)))?;
        match self.recv()? {
            ChannelMessage::ConsumeOk(tag, rx) => Ok((tag, rx)),
            ChannelMessage::Method(_) => Err(ErrorKind::FrameUnexpected)?,
        }
    }

    pub(super) fn call<M: IntoAmqpClass, T: TryFromAmqpClass>(&mut self, method: M) -> Result<T> {
        let buf = self.make_buf(method)?;
        self.call_rpc(IoLoopRpc::Send(buf))
    }

    pub(super) fn call_rpc<T: TryFromAmqpClass>(&mut self, rpc: IoLoopRpc) -> Result<T> {
        self.send(IoLoopMessage::Rpc(rpc))?;
        match self.recv()? {
            ChannelMessage::Method(method) => T::try_from(method),
            ChannelMessage::ConsumeOk(_, _) => Err(ErrorKind::FrameUnexpected)?,
        }
    }

    pub(super) fn send_nowait<M: IntoAmqpClass>(&mut self, method: M) -> Result<()> {
        let buf = self.make_buf(method)?;
        self.send_rpc_nowait(IoLoopRpc::Send(buf))
    }

    fn send_rpc_nowait(&mut self, rpc: IoLoopRpc) -> Result<()> {
        self.send(IoLoopMessage::Rpc(rpc))
    }

    pub(super) fn send_content_header(
        &mut self,
        class_id: u16,
        len: usize,
        properties: &AmqpProperties,
    ) -> Result<()> {
        debug_assert!(self.buf.is_empty());
        self.buf
            .push_content_header(self.channel_id, class_id, len, properties)?;
        let buf = self.buf.drain_into_new_buf();
        self.send_rpc_nowait(IoLoopRpc::Send(buf))
    }

    pub(super) fn send_content_body(&mut self, content: &[u8]) -> Result<()> {
        debug_assert!(self.buf.is_empty());
        self.buf.push_content_body(self.channel_id, content)?;
        let buf = self.buf.drain_into_new_buf();
        self.send_rpc_nowait(IoLoopRpc::Send(buf))
    }

    fn send(&mut self, message: IoLoopMessage) -> Result<()> {
        self.tx.send(message).map_err(|_| {
            // failed to send to the I/O thread; possible causes are:
            //   1. Server closed channel; we should see if there's a relevant message
            //      waiting for us on rx.
            //   2. I/O loop is actually gone.
            // In either case, recv() will return Err. If it doesn't, we got somehow
            // got a frame after a send failure - this should be impossible, but return
            // FrameUnexpected just in case.
            match self.recv() {
                Ok(_) => {
                    error!(
                        "internal error - received unexpected frame after I/O thread disappeared"
                    );
                    ErrorKind::FrameUnexpected.into()
                }
                Err(err) => err,
            }
        })
    }

    fn recv(&mut self) -> Result<ChannelMessage> {
        self.rx
            .recv()
            .map_err(|_| Error::from(ErrorKind::EventLoopDropped))?
    }
}

pub(super) struct IoLoopHandle0 {
    common: IoLoopHandle,
    pub(super) blocked_rx: CrossbeamReceiver<ConnectionBlockedNotification>,
}

impl IoLoopHandle0 {
    pub(super) fn new(
        common: IoLoopHandle,
        blocked_rx: CrossbeamReceiver<ConnectionBlockedNotification>,
    ) -> IoLoopHandle0 {
        IoLoopHandle0 { common, blocked_rx }
    }
}

impl Deref for IoLoopHandle0 {
    type Target = IoLoopHandle;

    fn deref(&self) -> &IoLoopHandle {
        &self.common
    }
}

impl DerefMut for IoLoopHandle0 {
    fn deref_mut(&mut self) -> &mut IoLoopHandle {
        &mut self.common
    }
}
