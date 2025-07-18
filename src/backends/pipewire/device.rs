use super::stream::StreamHandle;
use crate::backends::pipewire::error::PipewireError;
use crate::{
    AudioDevice, AudioInputCallback, AudioInputDevice, AudioOutputCallback, AudioOutputDevice,
    Channel, DeviceType, SendEverywhereButOnWeb, StreamConfig,
};
use pipewire::context::Context;
use pipewire::main_loop::MainLoop;
use pipewire::properties::Properties;
use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

pub struct PipewireDevice {
    pub(super) target_node: Option<u32>,
    pub device_type: DeviceType,
    pub object_serial: Option<String>,
    pub stream_name: Cow<'static, str>,
    pub stream_properties: HashMap<Vec<u8>, Vec<u8>>,
}

impl PipewireDevice {
    pub fn properties(&self) -> Result<Option<Properties>, PipewireError> {
        let Some(node_id) = self.target_node else {
            return Ok(None);
        };

        get_node_properties(node_id)
    }
}

impl AudioDevice for PipewireDevice {
    type Error = PipewireError;

    fn name(&self) -> Cow<str> {
        let Some(node_id) = self.target_node else {
            return Cow::Borrowed("Default");
        };
        match get_node_properties(node_id) {
            Ok(properties) => properties
                .and_then(|properties| {
                    properties
                        .get("node.name")
                        .map(|name| Cow::Owned(name.to_owned()))
                })
                .unwrap_or(Cow::Borrowed("Unknown")),
            Err(e) => {
                log::error!("Failed to get device name: {}", e);
                Cow::Borrowed("Error")
            }
        }
    }

    fn device_type(&self) -> DeviceType {
        self.device_type
    }

    fn channel_map(&self) -> impl IntoIterator<Item = Channel> {
        []
    }

    fn is_config_supported(&self, _config: &StreamConfig) -> bool {
        true
    }

    fn enumerate_configurations(&self) -> Option<impl IntoIterator<Item = StreamConfig>> {
        Some([])
    }
}

impl AudioInputDevice for PipewireDevice {
    type StreamHandle<Callback: AudioInputCallback> = StreamHandle<Callback>;

    fn default_input_config(&self) -> Result<StreamConfig, Self::Error> {
        Ok(StreamConfig {
            samplerate: 48000.0,
            channels: 0b11,
            exclusive: false,
            buffer_size_range: (None, None),
        })
    }

    fn create_input_stream<Callback: SendEverywhereButOnWeb + AudioInputCallback>(
        &self,
        stream_config: StreamConfig,
        callback: Callback,
    ) -> Result<Self::StreamHandle<Callback>, Self::Error> {
        StreamHandle::new_input(
            self.object_serial.clone(),
            &self.stream_name,
            stream_config,
            self.stream_properties.clone(),
            callback,
        )
    }
}

impl AudioOutputDevice for PipewireDevice {
    type StreamHandle<Callback: AudioOutputCallback> = StreamHandle<Callback>;

    fn default_output_config(&self) -> Result<StreamConfig, Self::Error> {
        Ok(StreamConfig {
            samplerate: 48000.0,
            channels: 0b11,
            exclusive: false,
            buffer_size_range: (None, None),
        })
    }

    fn create_output_stream<Callback: SendEverywhereButOnWeb + AudioOutputCallback>(
        &self,
        stream_config: StreamConfig,
        callback: Callback,
    ) -> Result<Self::StreamHandle<Callback>, Self::Error> {
        StreamHandle::new_output(
            self.object_serial.clone(),
            &self.stream_name,
            stream_config,
            self.stream_properties.clone(),
            callback,
        )
    }
}

impl PipewireDevice {
    /// Name to be used for the next stream created with [`AudioInputDevice::create_input_stream()`]
    /// or [`AudioOutputDevice::create_output_stream()`].
    pub fn with_stream_name(&mut self, name: impl Into<Cow<'static, str>>) {
        self.stream_name = name.into();
    }

    /// Properties for the next stream created with [`AudioInputDevice::create_input_stream()`]
    /// or [`AudioOutputDevice::create_output_stream()`].
    /// These correspond to [`pipewire::properties::Properties`] but need to be passed in a
    /// [`HashMap`] as the pipewire properties are not `Send`.
    pub fn with_stream_properties(&mut self, properties: HashMap<Vec<u8>, Vec<u8>>) {
        self.stream_properties = properties
    }
}

fn get_node_properties(node_id: u32) -> Result<Option<Properties>, PipewireError> {
    let mainloop = MainLoop::new(None)?;
    let context = Context::new(&mainloop)?;
    let core = context.connect(None)?;
    let registry = core.get_registry()?;

    // To comply with Rust's safety rules, we wrap this variable in an `Rc` and  a `Cell`.
    let done = Rc::new(Cell::new(false));

    // Create new reference for each variable so that they can be moved into the closure.
    let done_clone = done.clone();
    let loop_clone = mainloop.clone();

    // Trigger the sync event. The server's answer won't be processed until we start the main loop,
    // so we can safely do this before setting up a callback. This lets us avoid using a Cell.
    let pending = core.sync(0)?;

    let _listener_core = core
        .add_listener_local()
        .done(move |id, seq| {
            log::debug!("[Core/Done] id: {id} seq: {}", seq.seq());
            if id == pipewire::core::PW_ID_CORE && seq == pending {
                done_clone.set(true);
                loop_clone.quit();
            }
        })
        .register();

    let data = Rc::new(RefCell::new(None));
    let _listener_reg = registry
        .add_listener_local()
        .global({
            let data = data.clone();
            move |global| {
                if node_id == global.id {
                    if let Some(properties) = global.props {
                        let properties = Properties::from_dict(properties);
                        data.borrow_mut().replace(properties);
                    }
                }
            }
        })
        .register();

    while !done.get() {
        mainloop.run();
    }
    drop(_listener_core);
    drop(_listener_reg);
    Ok(Rc::into_inner(data).unwrap().into_inner())
}
