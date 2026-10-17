import datetime

from blue.events.v1 import events_pb2 as _events_pb2
from google.protobuf import timestamp_pb2 as _timestamp_pb2
from google.protobuf.internal import containers as _containers
from google.protobuf.internal import enum_type_wrapper as _enum_type_wrapper
from google.protobuf import descriptor as _descriptor
from google.protobuf import message as _message
from collections.abc import Iterable as _Iterable, Mapping as _Mapping
from typing import ClassVar as _ClassVar, Optional as _Optional, Union as _Union

DESCRIPTOR: _descriptor.FileDescriptor

class TurnState(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    TURN_STATE_UNSPECIFIED: _ClassVar[TurnState]
    TURN_STATE_QUEUED: _ClassVar[TurnState]
    TURN_STATE_RUNNING: _ClassVar[TurnState]
    TURN_STATE_COMPLETED: _ClassVar[TurnState]
    TURN_STATE_FAILED: _ClassVar[TurnState]
    TURN_STATE_INTERRUPTED: _ClassVar[TurnState]
TURN_STATE_UNSPECIFIED: TurnState
TURN_STATE_QUEUED: TurnState
TURN_STATE_RUNNING: TurnState
TURN_STATE_COMPLETED: TurnState
TURN_STATE_FAILED: TurnState
TURN_STATE_INTERRUPTED: TurnState

class Machine(_message.Message):
    __slots__ = ("id", "name", "online", "os", "last_seen_at")
    ID_FIELD_NUMBER: _ClassVar[int]
    NAME_FIELD_NUMBER: _ClassVar[int]
    ONLINE_FIELD_NUMBER: _ClassVar[int]
    OS_FIELD_NUMBER: _ClassVar[int]
    LAST_SEEN_AT_FIELD_NUMBER: _ClassVar[int]
    id: str
    name: str
    online: bool
    os: str
    last_seen_at: _timestamp_pb2.Timestamp
    def __init__(self, id: _Optional[str] = ..., name: _Optional[str] = ..., online: _Optional[bool] = ..., os: _Optional[str] = ..., last_seen_at: _Optional[_Union[datetime.datetime, _timestamp_pb2.Timestamp, _Mapping]] = ...) -> None: ...

class Session(_message.Message):
    __slots__ = ("id", "machine_id", "machine_name", "title", "state", "created_at", "last_activity_at", "current_turn")
    ID_FIELD_NUMBER: _ClassVar[int]
    MACHINE_ID_FIELD_NUMBER: _ClassVar[int]
    MACHINE_NAME_FIELD_NUMBER: _ClassVar[int]
    TITLE_FIELD_NUMBER: _ClassVar[int]
    STATE_FIELD_NUMBER: _ClassVar[int]
    CREATED_AT_FIELD_NUMBER: _ClassVar[int]
    LAST_ACTIVITY_AT_FIELD_NUMBER: _ClassVar[int]
    CURRENT_TURN_FIELD_NUMBER: _ClassVar[int]
    id: str
    machine_id: str
    machine_name: str
    title: str
    state: _events_pb2.SessionState
    created_at: _timestamp_pb2.Timestamp
    last_activity_at: _timestamp_pb2.Timestamp
    current_turn: Turn
    def __init__(self, id: _Optional[str] = ..., machine_id: _Optional[str] = ..., machine_name: _Optional[str] = ..., title: _Optional[str] = ..., state: _Optional[_Union[_events_pb2.SessionState, str]] = ..., created_at: _Optional[_Union[datetime.datetime, _timestamp_pb2.Timestamp, _Mapping]] = ..., last_activity_at: _Optional[_Union[datetime.datetime, _timestamp_pb2.Timestamp, _Mapping]] = ..., current_turn: _Optional[_Union[Turn, _Mapping]] = ...) -> None: ...

class Turn(_message.Message):
    __slots__ = ("id", "ordinal", "instructions", "state", "result_text", "started_at", "finished_at")
    ID_FIELD_NUMBER: _ClassVar[int]
    ORDINAL_FIELD_NUMBER: _ClassVar[int]
    INSTRUCTIONS_FIELD_NUMBER: _ClassVar[int]
    STATE_FIELD_NUMBER: _ClassVar[int]
    RESULT_TEXT_FIELD_NUMBER: _ClassVar[int]
    STARTED_AT_FIELD_NUMBER: _ClassVar[int]
    FINISHED_AT_FIELD_NUMBER: _ClassVar[int]
    id: str
    ordinal: int
    instructions: str
    state: TurnState
    result_text: str
    started_at: _timestamp_pb2.Timestamp
    finished_at: _timestamp_pb2.Timestamp
    def __init__(self, id: _Optional[str] = ..., ordinal: _Optional[int] = ..., instructions: _Optional[str] = ..., state: _Optional[_Union[TurnState, str]] = ..., result_text: _Optional[str] = ..., started_at: _Optional[_Union[datetime.datetime, _timestamp_pb2.Timestamp, _Mapping]] = ..., finished_at: _Optional[_Union[datetime.datetime, _timestamp_pb2.Timestamp, _Mapping]] = ...) -> None: ...

class Event(_message.Message):
    __slots__ = ("cursor", "occurred_at", "session", "machine_status")
    CURSOR_FIELD_NUMBER: _ClassVar[int]
    OCCURRED_AT_FIELD_NUMBER: _ClassVar[int]
    SESSION_FIELD_NUMBER: _ClassVar[int]
    MACHINE_STATUS_FIELD_NUMBER: _ClassVar[int]
    cursor: int
    occurred_at: _timestamp_pb2.Timestamp
    session: SessionEventEnvelope
    machine_status: MachineStatus
    def __init__(self, cursor: _Optional[int] = ..., occurred_at: _Optional[_Union[datetime.datetime, _timestamp_pb2.Timestamp, _Mapping]] = ..., session: _Optional[_Union[SessionEventEnvelope, _Mapping]] = ..., machine_status: _Optional[_Union[MachineStatus, _Mapping]] = ...) -> None: ...

class SessionEventEnvelope(_message.Message):
    __slots__ = ("session_id", "turn_id", "event")
    SESSION_ID_FIELD_NUMBER: _ClassVar[int]
    TURN_ID_FIELD_NUMBER: _ClassVar[int]
    EVENT_FIELD_NUMBER: _ClassVar[int]
    session_id: str
    turn_id: str
    event: _events_pb2.SessionEvent
    def __init__(self, session_id: _Optional[str] = ..., turn_id: _Optional[str] = ..., event: _Optional[_Union[_events_pb2.SessionEvent, _Mapping]] = ...) -> None: ...

class MachineStatus(_message.Message):
    __slots__ = ("machine_id", "machine_name", "online")
    MACHINE_ID_FIELD_NUMBER: _ClassVar[int]
    MACHINE_NAME_FIELD_NUMBER: _ClassVar[int]
    ONLINE_FIELD_NUMBER: _ClassVar[int]
    machine_id: str
    machine_name: str
    online: bool
    def __init__(self, machine_id: _Optional[str] = ..., machine_name: _Optional[str] = ..., online: _Optional[bool] = ...) -> None: ...

class AmbiguousTarget(_message.Message):
    __slots__ = ("query", "candidates")
    QUERY_FIELD_NUMBER: _ClassVar[int]
    CANDIDATES_FIELD_NUMBER: _ClassVar[int]
    query: str
    candidates: _containers.RepeatedScalarFieldContainer[str]
    def __init__(self, query: _Optional[str] = ..., candidates: _Optional[_Iterable[str]] = ...) -> None: ...

class ListMachinesRequest(_message.Message):
    __slots__ = ()
    def __init__(self) -> None: ...

class ListMachinesResponse(_message.Message):
    __slots__ = ("machines",)
    MACHINES_FIELD_NUMBER: _ClassVar[int]
    machines: _containers.RepeatedCompositeFieldContainer[Machine]
    def __init__(self, machines: _Optional[_Iterable[_Union[Machine, _Mapping]]] = ...) -> None: ...

class ListSessionsRequest(_message.Message):
    __slots__ = ("machine", "active_only")
    MACHINE_FIELD_NUMBER: _ClassVar[int]
    ACTIVE_ONLY_FIELD_NUMBER: _ClassVar[int]
    machine: str
    active_only: bool
    def __init__(self, machine: _Optional[str] = ..., active_only: _Optional[bool] = ...) -> None: ...

class ListSessionsResponse(_message.Message):
    __slots__ = ("sessions",)
    SESSIONS_FIELD_NUMBER: _ClassVar[int]
    sessions: _containers.RepeatedCompositeFieldContainer[Session]
    def __init__(self, sessions: _Optional[_Iterable[_Union[Session, _Mapping]]] = ...) -> None: ...

class CreateSessionRequest(_message.Message):
    __slots__ = ("machine", "instructions", "title")
    MACHINE_FIELD_NUMBER: _ClassVar[int]
    INSTRUCTIONS_FIELD_NUMBER: _ClassVar[int]
    TITLE_FIELD_NUMBER: _ClassVar[int]
    machine: str
    instructions: str
    title: str
    def __init__(self, machine: _Optional[str] = ..., instructions: _Optional[str] = ..., title: _Optional[str] = ...) -> None: ...

class CreateSessionResponse(_message.Message):
    __slots__ = ("session", "turn_id")
    SESSION_FIELD_NUMBER: _ClassVar[int]
    TURN_ID_FIELD_NUMBER: _ClassVar[int]
    session: Session
    turn_id: str
    def __init__(self, session: _Optional[_Union[Session, _Mapping]] = ..., turn_id: _Optional[str] = ...) -> None: ...

class SendPromptRequest(_message.Message):
    __slots__ = ("session_id", "instructions")
    SESSION_ID_FIELD_NUMBER: _ClassVar[int]
    INSTRUCTIONS_FIELD_NUMBER: _ClassVar[int]
    session_id: str
    instructions: str
    def __init__(self, session_id: _Optional[str] = ..., instructions: _Optional[str] = ...) -> None: ...

class SendPromptResponse(_message.Message):
    __slots__ = ("turn_id",)
    TURN_ID_FIELD_NUMBER: _ClassVar[int]
    turn_id: str
    def __init__(self, turn_id: _Optional[str] = ...) -> None: ...

class InterruptSessionRequest(_message.Message):
    __slots__ = ("session_id",)
    SESSION_ID_FIELD_NUMBER: _ClassVar[int]
    session_id: str
    def __init__(self, session_id: _Optional[str] = ...) -> None: ...

class InterruptSessionResponse(_message.Message):
    __slots__ = ()
    def __init__(self) -> None: ...

class GetSessionRequest(_message.Message):
    __slots__ = ("session_id", "recent_events")
    SESSION_ID_FIELD_NUMBER: _ClassVar[int]
    RECENT_EVENTS_FIELD_NUMBER: _ClassVar[int]
    session_id: str
    recent_events: int
    def __init__(self, session_id: _Optional[str] = ..., recent_events: _Optional[int] = ...) -> None: ...

class GetSessionResponse(_message.Message):
    __slots__ = ("session", "recent_events")
    SESSION_FIELD_NUMBER: _ClassVar[int]
    RECENT_EVENTS_FIELD_NUMBER: _ClassVar[int]
    session: Session
    recent_events: _containers.RepeatedCompositeFieldContainer[Event]
    def __init__(self, session: _Optional[_Union[Session, _Mapping]] = ..., recent_events: _Optional[_Iterable[_Union[Event, _Mapping]]] = ...) -> None: ...

class SubscribeRequest(_message.Message):
    __slots__ = ("cursor", "session_ids", "include_raw")
    CURSOR_FIELD_NUMBER: _ClassVar[int]
    SESSION_IDS_FIELD_NUMBER: _ClassVar[int]
    INCLUDE_RAW_FIELD_NUMBER: _ClassVar[int]
    cursor: int
    session_ids: _containers.RepeatedScalarFieldContainer[str]
    include_raw: bool
    def __init__(self, cursor: _Optional[int] = ..., session_ids: _Optional[_Iterable[str]] = ..., include_raw: _Optional[bool] = ...) -> None: ...

class SubscribeResponse(_message.Message):
    __slots__ = ("event",)
    EVENT_FIELD_NUMBER: _ClassVar[int]
    event: Event
    def __init__(self, event: _Optional[_Union[Event, _Mapping]] = ...) -> None: ...
