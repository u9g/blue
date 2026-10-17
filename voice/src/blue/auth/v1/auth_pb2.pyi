import datetime

from google.protobuf import timestamp_pb2 as _timestamp_pb2
from google.protobuf import descriptor as _descriptor
from google.protobuf import message as _message
from collections.abc import Mapping as _Mapping
from typing import ClassVar as _ClassVar, Optional as _Optional, Union as _Union

DESCRIPTOR: _descriptor.FileDescriptor

class LoginRequest(_message.Message):
    __slots__ = ("username", "password")
    USERNAME_FIELD_NUMBER: _ClassVar[int]
    PASSWORD_FIELD_NUMBER: _ClassVar[int]
    username: str
    password: str
    def __init__(self, username: _Optional[str] = ..., password: _Optional[str] = ...) -> None: ...

class LoginResponse(_message.Message):
    __slots__ = ("token", "expires_at")
    TOKEN_FIELD_NUMBER: _ClassVar[int]
    EXPIRES_AT_FIELD_NUMBER: _ClassVar[int]
    token: str
    expires_at: _timestamp_pb2.Timestamp
    def __init__(self, token: _Optional[str] = ..., expires_at: _Optional[_Union[datetime.datetime, _timestamp_pb2.Timestamp, _Mapping]] = ...) -> None: ...

class CreateLiveKitTokenRequest(_message.Message):
    __slots__ = ("room_name",)
    ROOM_NAME_FIELD_NUMBER: _ClassVar[int]
    room_name: str
    def __init__(self, room_name: _Optional[str] = ...) -> None: ...

class CreateLiveKitTokenResponse(_message.Message):
    __slots__ = ("token", "livekit_url", "room_name")
    TOKEN_FIELD_NUMBER: _ClassVar[int]
    LIVEKIT_URL_FIELD_NUMBER: _ClassVar[int]
    ROOM_NAME_FIELD_NUMBER: _ClassVar[int]
    token: str
    livekit_url: str
    room_name: str
    def __init__(self, token: _Optional[str] = ..., livekit_url: _Optional[str] = ..., room_name: _Optional[str] = ...) -> None: ...

class CreatePairingCodeRequest(_message.Message):
    __slots__ = ()
    def __init__(self) -> None: ...

class CreatePairingCodeResponse(_message.Message):
    __slots__ = ("code", "expires_at")
    CODE_FIELD_NUMBER: _ClassVar[int]
    EXPIRES_AT_FIELD_NUMBER: _ClassVar[int]
    code: str
    expires_at: _timestamp_pb2.Timestamp
    def __init__(self, code: _Optional[str] = ..., expires_at: _Optional[_Union[datetime.datetime, _timestamp_pb2.Timestamp, _Mapping]] = ...) -> None: ...

class ClaimPairingCodeRequest(_message.Message):
    __slots__ = ("code", "machine_name")
    CODE_FIELD_NUMBER: _ClassVar[int]
    MACHINE_NAME_FIELD_NUMBER: _ClassVar[int]
    code: str
    machine_name: str
    def __init__(self, code: _Optional[str] = ..., machine_name: _Optional[str] = ...) -> None: ...

class ClaimPairingCodeResponse(_message.Message):
    __slots__ = ("machine_id", "machine_token")
    MACHINE_ID_FIELD_NUMBER: _ClassVar[int]
    MACHINE_TOKEN_FIELD_NUMBER: _ClassVar[int]
    machine_id: str
    machine_token: str
    def __init__(self, machine_id: _Optional[str] = ..., machine_token: _Optional[str] = ...) -> None: ...

class RevokeMachineRequest(_message.Message):
    __slots__ = ("machine",)
    MACHINE_FIELD_NUMBER: _ClassVar[int]
    machine: str
    def __init__(self, machine: _Optional[str] = ...) -> None: ...

class RevokeMachineResponse(_message.Message):
    __slots__ = ()
    def __init__(self) -> None: ...
