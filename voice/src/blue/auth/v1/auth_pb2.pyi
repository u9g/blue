import datetime

from google.protobuf import timestamp_pb2 as _timestamp_pb2
from google.protobuf import descriptor as _descriptor
from google.protobuf import message as _message
from collections.abc import Mapping as _Mapping
from typing import ClassVar as _ClassVar, Optional as _Optional, Union as _Union

DESCRIPTOR: _descriptor.FileDescriptor

class User(_message.Message):
    __slots__ = ("github_id", "github_login", "name", "avatar_url")
    GITHUB_ID_FIELD_NUMBER: _ClassVar[int]
    GITHUB_LOGIN_FIELD_NUMBER: _ClassVar[int]
    NAME_FIELD_NUMBER: _ClassVar[int]
    AVATAR_URL_FIELD_NUMBER: _ClassVar[int]
    github_id: int
    github_login: str
    name: str
    avatar_url: str
    def __init__(self, github_id: _Optional[int] = ..., github_login: _Optional[str] = ..., name: _Optional[str] = ..., avatar_url: _Optional[str] = ...) -> None: ...

class LoginWithGitHubRequest(_message.Message):
    __slots__ = ("client_name",)
    CLIENT_NAME_FIELD_NUMBER: _ClassVar[int]
    client_name: str
    def __init__(self, client_name: _Optional[str] = ...) -> None: ...

class LoginWithGitHubResponse(_message.Message):
    __slots__ = ("device_code", "complete")
    DEVICE_CODE_FIELD_NUMBER: _ClassVar[int]
    COMPLETE_FIELD_NUMBER: _ClassVar[int]
    device_code: DeviceCode
    complete: LoginComplete
    def __init__(self, device_code: _Optional[_Union[DeviceCode, _Mapping]] = ..., complete: _Optional[_Union[LoginComplete, _Mapping]] = ...) -> None: ...

class DeviceCode(_message.Message):
    __slots__ = ("user_code", "verification_uri", "expires_at")
    USER_CODE_FIELD_NUMBER: _ClassVar[int]
    VERIFICATION_URI_FIELD_NUMBER: _ClassVar[int]
    EXPIRES_AT_FIELD_NUMBER: _ClassVar[int]
    user_code: str
    verification_uri: str
    expires_at: _timestamp_pb2.Timestamp
    def __init__(self, user_code: _Optional[str] = ..., verification_uri: _Optional[str] = ..., expires_at: _Optional[_Union[datetime.datetime, _timestamp_pb2.Timestamp, _Mapping]] = ...) -> None: ...

class LoginComplete(_message.Message):
    __slots__ = ("session_token", "expires_at", "user")
    SESSION_TOKEN_FIELD_NUMBER: _ClassVar[int]
    EXPIRES_AT_FIELD_NUMBER: _ClassVar[int]
    USER_FIELD_NUMBER: _ClassVar[int]
    session_token: str
    expires_at: _timestamp_pb2.Timestamp
    user: User
    def __init__(self, session_token: _Optional[str] = ..., expires_at: _Optional[_Union[datetime.datetime, _timestamp_pb2.Timestamp, _Mapping]] = ..., user: _Optional[_Union[User, _Mapping]] = ...) -> None: ...

class LogoutRequest(_message.Message):
    __slots__ = ()
    def __init__(self) -> None: ...

class LogoutResponse(_message.Message):
    __slots__ = ()
    def __init__(self) -> None: ...

class GetCurrentUserRequest(_message.Message):
    __slots__ = ()
    def __init__(self) -> None: ...

class GetCurrentUserResponse(_message.Message):
    __slots__ = ("user",)
    USER_FIELD_NUMBER: _ClassVar[int]
    user: User
    def __init__(self, user: _Optional[_Union[User, _Mapping]] = ...) -> None: ...

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
