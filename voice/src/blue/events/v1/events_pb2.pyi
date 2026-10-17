from google.protobuf.internal import enum_type_wrapper as _enum_type_wrapper
from google.protobuf import descriptor as _descriptor
from google.protobuf import message as _message
from collections.abc import Mapping as _Mapping
from typing import ClassVar as _ClassVar, Optional as _Optional, Union as _Union

DESCRIPTOR: _descriptor.FileDescriptor

class SessionState(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    SESSION_STATE_UNSPECIFIED: _ClassVar[SessionState]
    SESSION_STATE_STARTING: _ClassVar[SessionState]
    SESSION_STATE_IDLE: _ClassVar[SessionState]
    SESSION_STATE_RUNNING: _ClassVar[SessionState]
    SESSION_STATE_CLOSED: _ClassVar[SessionState]
    SESSION_STATE_FAILED: _ClassVar[SessionState]
SESSION_STATE_UNSPECIFIED: SessionState
SESSION_STATE_STARTING: SessionState
SESSION_STATE_IDLE: SessionState
SESSION_STATE_RUNNING: SessionState
SESSION_STATE_CLOSED: SessionState
SESSION_STATE_FAILED: SessionState

class SessionEvent(_message.Message):
    __slots__ = ("session_started", "turn_started", "assistant_text", "tool_started", "tool_finished", "turn_completed", "turn_failed", "turn_interrupted", "session_state_changed", "raw_sdk_message")
    SESSION_STARTED_FIELD_NUMBER: _ClassVar[int]
    TURN_STARTED_FIELD_NUMBER: _ClassVar[int]
    ASSISTANT_TEXT_FIELD_NUMBER: _ClassVar[int]
    TOOL_STARTED_FIELD_NUMBER: _ClassVar[int]
    TOOL_FINISHED_FIELD_NUMBER: _ClassVar[int]
    TURN_COMPLETED_FIELD_NUMBER: _ClassVar[int]
    TURN_FAILED_FIELD_NUMBER: _ClassVar[int]
    TURN_INTERRUPTED_FIELD_NUMBER: _ClassVar[int]
    SESSION_STATE_CHANGED_FIELD_NUMBER: _ClassVar[int]
    RAW_SDK_MESSAGE_FIELD_NUMBER: _ClassVar[int]
    session_started: SessionStarted
    turn_started: TurnStarted
    assistant_text: AssistantText
    tool_started: ToolStarted
    tool_finished: ToolFinished
    turn_completed: TurnCompleted
    turn_failed: TurnFailed
    turn_interrupted: TurnInterrupted
    session_state_changed: SessionStateChanged
    raw_sdk_message: RawSdkMessage
    def __init__(self, session_started: _Optional[_Union[SessionStarted, _Mapping]] = ..., turn_started: _Optional[_Union[TurnStarted, _Mapping]] = ..., assistant_text: _Optional[_Union[AssistantText, _Mapping]] = ..., tool_started: _Optional[_Union[ToolStarted, _Mapping]] = ..., tool_finished: _Optional[_Union[ToolFinished, _Mapping]] = ..., turn_completed: _Optional[_Union[TurnCompleted, _Mapping]] = ..., turn_failed: _Optional[_Union[TurnFailed, _Mapping]] = ..., turn_interrupted: _Optional[_Union[TurnInterrupted, _Mapping]] = ..., session_state_changed: _Optional[_Union[SessionStateChanged, _Mapping]] = ..., raw_sdk_message: _Optional[_Union[RawSdkMessage, _Mapping]] = ...) -> None: ...

class SessionStarted(_message.Message):
    __slots__ = ("claude_session_id", "model", "cwd")
    CLAUDE_SESSION_ID_FIELD_NUMBER: _ClassVar[int]
    MODEL_FIELD_NUMBER: _ClassVar[int]
    CWD_FIELD_NUMBER: _ClassVar[int]
    claude_session_id: str
    model: str
    cwd: str
    def __init__(self, claude_session_id: _Optional[str] = ..., model: _Optional[str] = ..., cwd: _Optional[str] = ...) -> None: ...

class TurnStarted(_message.Message):
    __slots__ = ("turn_id",)
    TURN_ID_FIELD_NUMBER: _ClassVar[int]
    turn_id: str
    def __init__(self, turn_id: _Optional[str] = ...) -> None: ...

class AssistantText(_message.Message):
    __slots__ = ("text",)
    TEXT_FIELD_NUMBER: _ClassVar[int]
    text: str
    def __init__(self, text: _Optional[str] = ...) -> None: ...

class ToolStarted(_message.Message):
    __slots__ = ("tool_use_id", "tool", "summary")
    TOOL_USE_ID_FIELD_NUMBER: _ClassVar[int]
    TOOL_FIELD_NUMBER: _ClassVar[int]
    SUMMARY_FIELD_NUMBER: _ClassVar[int]
    tool_use_id: str
    tool: str
    summary: str
    def __init__(self, tool_use_id: _Optional[str] = ..., tool: _Optional[str] = ..., summary: _Optional[str] = ...) -> None: ...

class ToolFinished(_message.Message):
    __slots__ = ("tool_use_id", "is_error")
    TOOL_USE_ID_FIELD_NUMBER: _ClassVar[int]
    IS_ERROR_FIELD_NUMBER: _ClassVar[int]
    tool_use_id: str
    is_error: bool
    def __init__(self, tool_use_id: _Optional[str] = ..., is_error: _Optional[bool] = ...) -> None: ...

class TurnCompleted(_message.Message):
    __slots__ = ("turn_id", "result_text", "duration_ms", "num_turns", "cost_usd")
    TURN_ID_FIELD_NUMBER: _ClassVar[int]
    RESULT_TEXT_FIELD_NUMBER: _ClassVar[int]
    DURATION_MS_FIELD_NUMBER: _ClassVar[int]
    NUM_TURNS_FIELD_NUMBER: _ClassVar[int]
    COST_USD_FIELD_NUMBER: _ClassVar[int]
    turn_id: str
    result_text: str
    duration_ms: int
    num_turns: int
    cost_usd: float
    def __init__(self, turn_id: _Optional[str] = ..., result_text: _Optional[str] = ..., duration_ms: _Optional[int] = ..., num_turns: _Optional[int] = ..., cost_usd: _Optional[float] = ...) -> None: ...

class TurnFailed(_message.Message):
    __slots__ = ("turn_id", "reason")
    TURN_ID_FIELD_NUMBER: _ClassVar[int]
    REASON_FIELD_NUMBER: _ClassVar[int]
    turn_id: str
    reason: str
    def __init__(self, turn_id: _Optional[str] = ..., reason: _Optional[str] = ...) -> None: ...

class TurnInterrupted(_message.Message):
    __slots__ = ("turn_id",)
    TURN_ID_FIELD_NUMBER: _ClassVar[int]
    turn_id: str
    def __init__(self, turn_id: _Optional[str] = ...) -> None: ...

class SessionStateChanged(_message.Message):
    __slots__ = ("state",)
    STATE_FIELD_NUMBER: _ClassVar[int]
    state: SessionState
    def __init__(self, state: _Optional[_Union[SessionState, str]] = ...) -> None: ...

class RawSdkMessage(_message.Message):
    __slots__ = ("json",)
    JSON_FIELD_NUMBER: _ClassVar[int]
    json: str
    def __init__(self, json: _Optional[str] = ...) -> None: ...
