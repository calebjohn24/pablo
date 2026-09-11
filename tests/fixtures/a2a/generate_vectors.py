"""Regenerate wire examples through pinned SDK protobuf JSON, never hand-coded aliases."""
from importlib.metadata import version
from pathlib import Path
import json
from google.protobuf.json_format import MessageToDict, MessageToJson
from a2a.types import a2a_pb2 as p
assert version("a2a-sdk") == "1.0.2"
root = Path(__file__).parent
card = p.AgentCard(name="Independent A2A fixture", description="Untrusted remote description; never local authority.", version="1.0.0",
    supported_interfaces=[p.AgentInterface(url="https://agent.example.test/rpc",protocol_binding="JSONRPC",protocol_version="1.0")],
    capabilities=p.AgentCapabilities(streaming=True),default_input_modes=["text/plain","application/json","application/octet-stream"],default_output_modes=["text/plain","application/json","application/octet-stream"],
    skills=[p.AgentSkill(id="echo",name="Echo",description="Return only explicitly submitted input.",tags=["fixture"])])
root.joinpath("card.json").write_text(MessageToJson(card)+"\n")
message = p.Message(message_id="local-message",role=p.ROLE_USER,parts=[p.Part(text="explicit selected input")])
reply = p.Message(message_id="remote-message",context_id="remote-context",role=p.ROLE_AGENT,parts=[p.Part(text="selected result")])
artifact = p.Artifact(artifact_id="remote-artifact",parts=[p.Part(text="selected result")])
task = p.Task(id="remote-task",context_id="remote-context",status=p.TaskStatus(state=p.TASK_STATE_COMPLETED),artifacts=[artifact])
request = p.SendMessageRequest(message=message,configuration=p.SendMessageConfiguration(accepted_output_modes=["text/plain","application/json"],history_length=0))
values = {
    "send": {"jsonrpc":"2.0","id":"rpc-send","method":"SendMessage","params":MessageToDict(request)},
    "stream": {"jsonrpc":"2.0","id":"rpc-stream","method":"SendStreamingMessage","params":MessageToDict(request)},
    "cancel": {"jsonrpc":"2.0","id":"rpc-cancel","method":"CancelTask","params":MessageToDict(p.CancelTaskRequest(id="remote-task"))},
    "cancel_result": MessageToDict(task),
    "message_result": MessageToDict(p.SendMessageResponse(message=reply)),
    "task_result": MessageToDict(p.SendMessageResponse(task=task)),
    "stream_message": MessageToDict(p.StreamResponse(message=reply)),
    "stream_task": MessageToDict(p.StreamResponse(task=task)),
    "stream_status": MessageToDict(p.StreamResponse(status_update=p.TaskStatusUpdateEvent(task_id="remote-task",context_id="remote-context",status=p.TaskStatus(state=p.TASK_STATE_WORKING)))),
    "stream_artifact": MessageToDict(p.StreamResponse(artifact_update=p.TaskArtifactUpdateEvent(task_id="remote-task",context_id="remote-context",artifact=artifact,last_chunk=True))),
}
file_message=p.Message(message_id="local-message",role=p.ROLE_USER,parts=[p.Part(raw=b"\x00\xffsynthetic file",media_type="application/octet-stream",filename="fixture.bin")])
file_request=p.SendMessageRequest(message=file_message,configuration=p.SendMessageConfiguration(accepted_output_modes=["application/octet-stream"],history_length=0))
values["file_send"]={"jsonrpc":"2.0","id":"rpc-file","method":"SendMessage","params":MessageToDict(file_request)}
url_message=p.Message(message_id="local-message",role=p.ROLE_USER,parts=[p.Part(url="https://files.example.test/inert",media_type="application/octet-stream")])
values["url_send"]={"jsonrpc":"2.0","id":"rpc-url","method":"SendMessage","params":MessageToDict(p.SendMessageRequest(message=url_message,configuration=file_request.configuration))}
root.joinpath("wire.json").write_text(json.dumps(values,indent=2)+"\n")
