#![doc = include_str!("../README.md")]
#![doc = r#"<link rel="stylesheet" href="../gallery/pkg/demo.css"><script type="module" src="../gallery/pkg/demo-loader.js"></script>"#]

mod colors;
mod compiler;
mod default_shader;
mod shader_graph;

use compiler::ShaderCompiler;
use demo_common::Demo;
use iced::{
    Element, Length, Task, Theme, Vector, Widget,
    widget::{column, container, text},
};
use iced_nodegraph::{
    Ids, PinDirection, PinInfo, PinRef, PinSide, PinStatus, PinStyle, default_pin_style,
    edge as ng_edge, node as ng_node, node_pin,
};
use shader_graph::ShaderGraph;
use std::collections::HashSet;

pub fn main() -> iced::Result {
    let window_settings = iced::window::Settings::default();

    iced::application(Application::boot, Application::update, Application::view)
        .title("Visual Shader Editor - iced_nodegraph")
        .theme(Application::theme)
        .window(window_settings)
        .run()
}

/// The id vocabulary of this demo: indexed nodes and pins, unidentified edges,
/// and a `TypeId` pin payload carrying the socket type of each pin.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct TypedIds;

impl Ids for TypedIds {
    type NodeId = usize;
    type PinId = usize;
    type EdgeId = ();
    type AnchorId = usize;
    type Payload = std::any::TypeId;
}

#[derive(Debug, Clone)]
enum Message {
    EdgeConnected {
        from: PinRef<TypedIds>,
        to: PinRef<TypedIds>,
    },
    EdgeDisconnected {
        from: PinRef<TypedIds>,
        to: PinRef<TypedIds>,
    },
    SelectionChanged(Vec<usize>),
    NodesMoved {
        delta: Vector,
        indices: Vec<usize>,
    },
}

struct Application {
    shader_graph: ShaderGraph,
    compiled_shader: Option<String>,
    compilation_error: Option<String>,
    visual_edges: Vec<(PinRef<TypedIds>, PinRef<TypedIds>)>,
    current_theme: Theme,
    graph_selection: HashSet<usize>,
}

impl demo_common::Demo for Application {
    type Message = Message;

    fn boot() -> (Self, iced::Task<Message>) {
        let shader_graph = default_shader::create_default_graph();

        // Convert shader graph connections to visual edges
        // NodeGraph widget uses flat pin indices: [input0, input1, ..., output0, output1, ...]
        // ShaderGraph uses separate indices: from_socket = output index, to_socket = input index
        let visual_edges: Vec<(PinRef<TypedIds>, PinRef<TypedIds>)> = shader_graph
            .connections
            .iter()
            .filter_map(|conn| {
                // Get the nodes to find their input/output counts
                let from_node = shader_graph.nodes.iter().find(|n| n.id == conn.from_node)?;
                // Validate target node exists
                shader_graph.nodes.iter().find(|n| n.id == conn.to_node)?;

                // Find node indices (position in nodes vec)
                let from_node_idx = shader_graph
                    .nodes
                    .iter()
                    .position(|n| n.id == conn.from_node)?;
                let to_node_idx = shader_graph
                    .nodes
                    .iter()
                    .position(|n| n.id == conn.to_node)?;

                // from_socket is an output index -> visual pin = num_inputs + output_index
                let from_visual_pin = from_node.inputs.len() + conn.from_socket;

                // to_socket is an input index -> visual pin = input_index (inputs come first)
                let to_visual_pin = conn.to_socket;

                Some((
                    PinRef::new(from_node_idx, from_visual_pin),
                    PinRef::new(to_node_idx, to_visual_pin),
                ))
            })
            .collect();

        let mut app = Self {
            shader_graph,
            compiled_shader: None,
            compilation_error: None,
            visual_edges,
            current_theme: Theme::CatppuccinMocha,
            graph_selection: HashSet::new(),
        };

        app.recompile();

        (app, iced::Task::none())
    }

    fn update(&mut self, message: Message) -> iced::Task<Message> {
        match message {
            Message::EdgeConnected { from, to } => {
                // Store visual edge as-is
                self.visual_edges.push((from, to));

                // Convert visual pin indices to shader socket indices
                // First, gather the info we need
                let connection_info = {
                    let from_node_data = self.shader_graph.nodes.get(from.node_id);
                    let to_node_data = self.shader_graph.nodes.get(to.node_id);

                    if let (Some(from_node_data), Some(to_node_data)) =
                        (from_node_data, to_node_data)
                    {
                        // from.pin_id is visual index, output starts after inputs
                        let from_socket = from.pin_id.saturating_sub(from_node_data.inputs.len());
                        // to.pin_id is visual index, inputs come first so it's direct
                        let to_socket = to.pin_id;

                        if from_socket < from_node_data.outputs.len()
                            && to_socket < to_node_data.inputs.len()
                        {
                            Some((from_node_data.id, from_socket, to_node_data.id, to_socket))
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                };

                // Now apply the connection
                if let Some((from_id, from_socket, to_id, to_socket)) = connection_info {
                    self.shader_graph.add_connection(shader_graph::Connection {
                        from_node: from_id,
                        from_socket,
                        to_node: to_id,
                        to_socket,
                    });
                }
                self.recompile();
            }
            Message::EdgeDisconnected { from, to } => {
                self.visual_edges.retain(|(f, t)| !(f == &from && t == &to));

                // Convert visual pin indices to shader socket indices and
                // resolve graph indices to node ids, mirroring EdgeConnected.
                let from_node = self.shader_graph.nodes.get(from.node_id);
                let to_node = self.shader_graph.nodes.get(to.node_id);
                if let (Some(from_node), Some(to_node)) = (from_node, to_node) {
                    let from_id = from_node.id;
                    let to_id = to_node.id;
                    let from_socket = from.pin_id.saturating_sub(from_node.inputs.len());
                    let to_socket = to.pin_id;

                    self.shader_graph.connections.retain(|c| {
                        !(c.from_node == from_id
                            && c.from_socket == from_socket
                            && c.to_node == to_id
                            && c.to_socket == to_socket)
                    });
                }
                self.recompile();
                return Task::none();
            }
            Message::SelectionChanged(indices) => {
                self.graph_selection = indices.into_iter().collect();
            }
            Message::NodesMoved { delta, indices } => {
                for idx in indices {
                    if let Some(node) = self.shader_graph.get_node_by_index_mut(idx) {
                        node.position.x += delta.x;
                        node.position.y += delta.y;
                    }
                }
            }
        }

        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        let graph = ::iced_nodegraph::NodeGraph::<TypedIds, _, _, _>::new()
            .on_connect(|from, to| Message::EdgeConnected { from, to })
            .on_move(|delta, indices| Message::NodesMoved { delta, indices })
            .on_disconnect(|from, to| Message::EdgeDisconnected { from, to })
            .on_select(Message::SelectionChanged)
            .nodes(
                self.shader_graph
                    .nodes
                    .iter()
                    .enumerate()
                    .map(|(node_idx, node)| {
                        let node_content = create_node_widget(&node.node_type, &self.current_theme);
                        ng_node(node_idx, node.position, node_content)
                            .selected(self.graph_selection.contains(&node_idx))
                            .pin_style(pin_style)
                    }),
            )
            .edges(
                self.visual_edges
                    .iter()
                    .map(|(from, to)| ng_edge((), *from, *to)),
            );

        graph.boxed()
    }

    fn theme(&self) -> Theme {
        self.current_theme.clone()
    }

    fn set_theme(&mut self, theme: Theme) {
        self.current_theme = theme;
    }
}

/// Boots this demo for the gallery.
pub fn scene() -> (
    Box<dyn demo_common::Scene>,
    iced::Task<demo_common::SceneMessage>,
) {
    demo_common::erase::<Application>()
}

impl Application {
    fn recompile(&mut self) {
        match ShaderCompiler::compile(&self.shader_graph) {
            Ok(shader) => {
                self.compiled_shader = Some(shader);
                self.compilation_error = None;
            }
            Err(err) => {
                self.compiled_shader = None;
                self.compilation_error = Some(err.to_string());
            }
        }
    }
}

fn create_node_widget<'a>(
    node_type: &shader_graph::nodes::ShaderNodeType,
    theme: &'a Theme,
) -> iced::Element<'a, Message> {
    use iced_nodegraph::node_pin;

    let name = node_type.name();
    let inputs = node_type.inputs();
    let outputs = node_type.outputs();

    let palette = theme.palette();

    // Title bar - matching hello_world pattern exactly
    let title_bar = container(text(name).size(13).width(Length::Fill))
        .width(Length::Fill)
        .padding([2, 8])
        .style(move |_theme: &iced::Theme| container::Style {
            background: None,
            text_color: Some(palette.background.base.text),
            ..container::Style::default()
        });

    // Build pin list - must match hello_world's column![] macro structure
    // Pin IDs use sequential indices: inputs first (0..n), then outputs (n..n+m)
    let pin_section = if inputs.is_empty() && outputs.is_empty() {
        // No pins - minimal output indicator
        container(
            column![
                node_pin(
                    PinSide::Right,
                    0usize,
                    container(text("out").size(11)).padding([0, 8])
                )
                .direction(PinDirection::Output)
            ]
            .spacing(2),
        )
        .padding([6, 0])
    } else {
        // Build pins dynamically but wrap in container same way
        let mut pin_elements: Vec<iced::Element<'a, Message>> = Vec::new();
        let num_inputs = inputs.len();

        for (i, input) in inputs.into_iter().enumerate() {
            let label = input.name.clone();
            pin_elements.push(create_typed_pin(
                PinSide::Left,
                i, // Pin ID = input index
                label,
                PinDirection::Input,
                &input.socket_type,
            ));
        }

        for (i, output) in outputs.into_iter().enumerate() {
            let label = output.name.clone();
            pin_elements.push(create_typed_pin(
                PinSide::Right,
                num_inputs + i, // Pin ID = num_inputs + output index
                label,
                PinDirection::Output,
                &output.socket_type,
            ));
        }

        container(iced::widget::Column::with_children(pin_elements).spacing(2)).padding([6, 0])
    };

    column![title_bar, pin_section].width(160.0).boxed()
}

/// Colors a node's pins by their socket data-type marker.
fn pin_style(
    theme: &Theme,
    pin: &PinInfo<'_, TypedIds>,
    _other: Option<&PinInfo<'_, TypedIds>>,
    status: PinStatus,
) -> PinStyle {
    use std::any::TypeId;
    let ty = *pin.info();
    let color = if ty == TypeId::of::<colors::Float>() {
        colors::SOCKET_FLOAT
    } else if ty == TypeId::of::<colors::Vec2>() {
        colors::SOCKET_VEC2
    } else if ty == TypeId::of::<colors::Vec3>() {
        colors::SOCKET_VEC3
    } else {
        colors::SOCKET_VEC4
    };
    PinStyle {
        color: color.into(),
        ..default_pin_style(theme, status)
    }
}

/// Creates a typed pin element based on the socket type.
/// Uses marker types for TypeId-based connection matching.
fn create_typed_pin<'a, Message: Clone + 'a>(
    side: PinSide,
    pin_id: usize,
    label: String,
    direction: PinDirection,
    socket_type: &shader_graph::sockets::SocketType,
) -> Element<'a, Message> {
    use shader_graph::sockets::SocketType;

    let content = container(text(label).size(11)).padding([0, 8]);

    match socket_type {
        SocketType::Float => node_pin(side, pin_id, content)
            .direction(direction)
            .info(::std::any::TypeId::of::<colors::Float>())
            .boxed(),
        SocketType::Vec2 => node_pin(side, pin_id, content)
            .direction(direction)
            .info(::std::any::TypeId::of::<colors::Vec2>())
            .boxed(),
        SocketType::Vec3 => node_pin(side, pin_id, content)
            .direction(direction)
            .info(::std::any::TypeId::of::<colors::Vec3>())
            .boxed(),
        SocketType::Vec4 => node_pin(side, pin_id, content)
            .direction(direction)
            .info(::std::any::TypeId::of::<colors::Vec4>())
            .boxed(),
    }
}
