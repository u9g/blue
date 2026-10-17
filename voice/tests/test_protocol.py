"""The generated protobuf code imports cleanly and round-trips the messages the agent will use."""

from blue.client.v1 import client_pb2, client_pb2_grpc
from blue.events.v1 import events_pb2


def test_create_session_round_trip() -> None:
    request = client_pb2.CreateSessionRequest(
        machine="desktop",
        instructions="in the blue repo, add a health endpoint",
        title="Health endpoint",
    )
    decoded = client_pb2.CreateSessionRequest.FromString(request.SerializeToString())
    assert decoded == request
    assert decoded.title == "Health endpoint"


def test_session_event_oneof() -> None:
    event = client_pb2.Event(
        cursor=42,
        session=client_pb2.SessionEventEnvelope(
            session_id="s1",
            turn_id="t1",
            event=events_pb2.SessionEvent(
                turn_completed=events_pb2.TurnCompleted(turn_id="t1", result_text="Done.")
            ),
        ),
    )
    decoded = client_pb2.Event.FromString(event.SerializeToString())
    assert decoded.WhichOneof("kind") == "session"
    assert decoded.session.event.WhichOneof("kind") == "turn_completed"
    assert decoded.session.event.turn_completed.result_text == "Done."


def test_client_stub_exposes_every_rpc() -> None:
    stub_methods = {
        name for name in vars(client_pb2_grpc.ClientServiceServicer) if not name.startswith("_")
    }
    assert stub_methods == {
        "ListMachines",
        "ListSessions",
        "CreateSession",
        "SendPrompt",
        "GetSession",
        "Subscribe",
    }
