import { Cause, Schema } from "effect";

/** stdin/stdout broke, or a frame couldn't be decoded. */
export class ProtocolError extends Schema.TaggedError<ProtocolError>()("ProtocolError", {
  cause: Schema.Defect(),
}) {}

/** The daemon sent a Start the bridge can't run. */
export class InvalidStart extends Schema.TaggedError<InvalidStart>()("InvalidStart", {
  reason: Schema.String,
}) {}

/** The Agent SDK (or the Claude Code process behind it) failed. */
export class SdkError extends Schema.TaggedError<SdkError>()("SdkError", {
  cause: Schema.Defect(),
}) {}

export type BridgeError = ProtocolError | InvalidStart | SdkError;

/** The reason reported to the daemon in `Exited.error`. */
export const describeFailure = (cause: Cause.Cause<BridgeError>): string => {
  const error = Cause.squash(cause);
  if (error instanceof InvalidStart) return error.reason;
  if (error instanceof SdkError || error instanceof ProtocolError) {
    return error.cause instanceof Error ? error.cause.message : String(error.cause);
  }
  return error instanceof Error ? error.message : String(error);
};
