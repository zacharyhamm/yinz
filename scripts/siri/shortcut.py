"""Build the portable Shortcuts property list. Credentials are supplied only at pairing."""
from contextlib import contextmanager
import plistlib
import uuid


def workflow():
    actions = []

    def identifier(purpose):
        return str(uuid.uuid5(uuid.NAMESPACE_URL, f"yinz:siri:{len(actions)}:{purpose}")).upper()

    def action(name, **parameters):
        uid = identifier(name)
        actions.append({"WFWorkflowActionIdentifier": "is.workflow.actions." + name,
                        "WFWorkflowActionParameters": {**parameters, "UUID": uid}})
        return {"Type": "ActionOutput", "OutputUUID": uid, "OutputName": name}

    def attachment(ref):
        return {"WFSerializationType": "WFTextTokenAttachment", "Value": ref}

    def text(*parts):
        value = ""
        refs = {}
        for part in parts:
            if isinstance(part, str):
                value += part
            else:
                offset = len(value.encode("utf-16-le")) // 2
                refs[f"{{{offset}, 1}}"] = part
                value += "\ufffc"
        return {"WFSerializationType": "WFTextTokenString",
                "Value": {"string": value, "attachmentsByRange": refs}}

    def dictionary(values):
        items = [{"WFItemType": 0, "WFKey": text(k), "WFValue": text(*v)}
                 for k, v in values.items()]
        return {"WFSerializationType": "WFDictionaryFieldValue",
                "Value": {"WFDictionaryFieldValueItems": items}}

    def get(ref, key):
        return action("getvalueforkey", WFInput=attachment(ref), WFDictionaryKey=key)

    def set_var(name, ref):
        action("setvariable", WFVariableName=name, WFInput=attachment(ref))
        return {"Type": "Variable", "VariableName": name}

    @contextmanager
    def block(name, **parameters):
        group = identifier(name)
        action(name, GroupingIdentifier=group, WFControlFlowMode=0, **parameters)
        yield
        action(name, GroupingIdentifier=group, WFControlFlowMode=2)

    def condition(ref, code, **parameters):
        return block("conditional", WFCondition=code, WFInput={"Type": "Variable", "Variable": attachment(ref)}, **parameters)

    def speak(*parts):
        action("speaktext", WFText=text(*parts), WFSpeakTextWait=True)

    def post(origin, path, body, token=None):
        headers = {"Origin": [origin], "Content-Type": ["application/json"]}
        if token is not None:
            headers["Authorization"] = ["Bearer ", token]
        return action("downloadurl", WFURL=text(origin, path), WFHTTPMethod="POST",
                      WFHTTPHeaders=dictionary(headers), WFHTTPBodyType="JSON",
                      WFJSONValues=dictionary(body))

    device = action("getdevicedetails", WFDeviceDetail="Device Name")
    filename = text("Talk to Yinz-", device, ".json")

    def save(origin, token, cursor, pending=""):
        record = action("dictionary", WFItems=dictionary({"origin": [origin], "token": [token],
                                                          "cursor": [cursor], "pending": [pending]}))
        # Coerce the dictionary to JSON text before saving it.
        content = action("gettext", WFText=text(record))
        action("documentpicker.save", WFInput=attachment(content), WFAskWhereToSave=False,
               WFFileDestinationPath=filename, WFSaveFileOverwrite=True)

    incoming = {"Type": "ExtensionInput"}
    with condition(incoming, 100):  # has any value
        setup = action("detect.dictionary", WFInput=attachment(incoming))
        origin = get(setup, "origin")
        code = get(setup, "code")
        pair = post(origin, "/api/siri/pair", {"code": [code]})
        token = get(pair, "token")
        with condition(token, 101):  # has no value
            speak("Pairing expired. Open Add to Siri in Yinz to get a new pairing link.")
            action("exit")
        save(origin, token, get(pair, "request_id"))
        speak("Paired. Say Siri, Talk to Yinz.")
        action("exit")

    config = action("documentpicker.open", WFShowFilePicker=False, WFGetFilePath=filename,
                    WFFileErrorIfNotFound=False)
    with condition(config, 101):
        speak("Open Add to Siri in the Yinz menu and pair this phone.")
        action("exit")
    record = action("detect.dictionary", WFInput=attachment(config))
    origin, token = get(record, "origin"), get(record, "token")
    cursor = set_var("Request", get(record, "cursor"))
    pending = set_var("Command", get(record, "pending"))
    with block("repeat.count", WFRepeatCount=20):
        with condition(pending, 101):
            dictated = action("dictatetext", WFSpeechLanguage="en-US")
            set_var("Command", dictated)
            with condition(pending, 101):
                action("exit")
            save(origin, token, cursor, pending)
        reply = post(origin, "/api/siri/command", {"command": [pending], "request_id": [cursor]}, token)
        next_id = get(reply, "request_id")
        with condition(next_id, 101):
            error = get(reply, "error")
            with condition(error, 100):
                for rejected in ("invalid_dictation", "body_too_large"):
                    with condition(get(error, "code"), 4, WFConditionalActionString=rejected):
                        save(origin, token, cursor)
                        speak("Run Talk to Yinz again and dictate a shorter replacement command.")
                        action("exit")
                # Nothing ran. Adopt the server's cursor and any command it is still finishing,
                # so the next run completes that command or dictates anew instead of replaying.
                with condition(get(error, "code"), 4, WFConditionalActionString="stale_siri_request"):
                    save(origin, token, get(reply, "cursor"), get(reply, "pending"))
                    speak("Yinz had moved on, so that command was not sent. Run Talk to Yinz again to continue.")
                    action("exit")
            speak("Yinz could not confirm the command. Run Talk to Yinz again to retry. If your sign-in expired, pair again.")
            action("exit")
        set_var("Request", next_id)
        set_var("Command", action("gettext", WFText=""))
        save(origin, token, cursor)
        speak(get(reply, "speech"))
        path = get(reply, "path")
        with condition(path, 100):
            action("openurl", WFInput=text(origin, path))
        # The server sends 'yes' for a follow-up, otherwise an empty string.
        with condition(get(reply, "follow_up"), 101):
            action("exit")

    return plistlib.dumps({
        "WFWorkflowName": "Talk to Yinz", "WFWorkflowClientVersion": "4030.0.4",
        "WFWorkflowMinimumClientVersion": 900, "WFWorkflowMinimumClientVersionString": "900",
        "WFWorkflowInputContentItemClasses": ["WFStringContentItem"], "WFWorkflowTypes": [],
        "WFWorkflowIcon": {"WFWorkflowIconStartColor": 4282601983, "WFWorkflowIconGlyphNumber": 61440},
        "WFWorkflowActions": actions,
    }, fmt=plistlib.FMT_BINARY)
