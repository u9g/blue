"""Blue's LiveKit voice agent.

The voice pipeline follows LiveKit's agent-starter-python defaults (LiveKit Inference STT/LLM/TTS,
LiveKit turn detector, ai-coustics noise cancellation). Hub tools arrive in phase 3.

Run locally:  uv run python -m blue_voice.agent dev
"""

import logging
import textwrap

from dotenv import load_dotenv
from livekit.agents import (
    Agent,
    AgentServer,
    AgentSession,
    JobContext,
    TurnHandlingOptions,
    cli,
    inference,
    room_io,
)
from livekit.plugins import ai_coustics

logger = logging.getLogger("blue-voice")

load_dotenv(".env.local")

AGENT_NAME = "blue-voice"

INSTRUCTIONS = textwrap.dedent(
    """\
    You are Blue, the voice front end to Claude Code agents running on the user's own machines.
    You do not write code yourself; you dispatch work to those agents and report back.

    You are speaking through text-to-speech:
    - Respond in plain text only. No markdown, lists, code, or emojis.
    - Keep replies to one or two sentences unless asked for more.
    - Never read code, diffs, or file paths character by character.

    Your tools for reaching the user's machines are not connected yet. If asked to do work,
    say briefly that dispatching isn't wired up yet.
    """
)


class Blue(Agent):
    def __init__(self) -> None:
        super().__init__(
            llm=inference.LLM(model="google/gemma-4-31b-it"),
            instructions=INSTRUCTIONS,
        )


server = AgentServer()


@server.rtc_session(agent_name=AGENT_NAME)
async def entrypoint(ctx: JobContext) -> None:
    ctx.log_context_fields = {"room": ctx.room.name}

    session = AgentSession(
        stt=inference.STT(model="assemblyai/universal-3-6-pro", language="en"),
        tts=inference.TTS(model="fishaudio/s2.1-pro", voice="fa4c9eb3dccc4806b382b40d61c6b10a"),
        turn_handling=TurnHandlingOptions(
            turn_detection=inference.TurnDetector(),
            interruption={"mode": "adaptive"},
            preemptive_generation={"enabled": True},
        ),
    )

    await session.start(
        agent=Blue(),
        room=ctx.room,
        room_options=room_io.RoomOptions(
            audio_input=room_io.AudioInputOptions(
                noise_cancellation=ai_coustics.audio_enhancement(
                    model=ai_coustics.EnhancerModel.QUAIL_VF_S
                ),
            ),
        ),
    )
    await ctx.connect()


if __name__ == "__main__":
    cli.run_app(server)
