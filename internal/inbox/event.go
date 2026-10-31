package inbox

import (
	"bytes"
	"encoding/json"
	"errors"
	"math"
	"strconv"
	"strings"
	"time"
)

var errEvent = errors.New("invalid event envelope")

type event struct {
	ID   string
	Test bool
}
type object map[string]json.RawMessage

func objectValue(raw []byte) (object, error) {
	var o object
	if len(raw) == 0 || json.Unmarshal(raw, &o) != nil || o == nil {
		return nil, errEvent
	}
	return o, nil
}
func stringValue(o object, key string, nonempty bool) (string, error) {
	raw, ok := o[key]
	var value string
	if !ok || bytes.Equal(bytes.TrimSpace(raw), []byte("null")) || json.Unmarshal(raw, &value) != nil || (nonempty && strings.TrimSpace(value) == "") {
		return "", errEvent
	}
	return value, nil
}
func arrayValue(o object, key string) ([]json.RawMessage, error) {
	raw, ok := o[key]
	var a []json.RawMessage
	if !ok || bytes.Equal(bytes.TrimSpace(raw), []byte("null")) || json.Unmarshal(raw, &a) != nil {
		return nil, errEvent
	}
	return a, nil
}
func nullableString(o object, key string) error {
	raw, ok := o[key]
	if !ok {
		return errEvent
	}
	if bytes.Equal(bytes.TrimSpace(raw), []byte("null")) {
		return nil
	}
	_, e := stringValue(o, key, false)
	return e
}
func numberValue(o object, key string, nullable bool) (float64, error) {
	raw, ok := o[key]
	if !ok {
		return 0, errEvent
	}
	if bytes.Equal(bytes.TrimSpace(raw), []byte("null")) {
		if nullable {
			return 0, nil
		}
		return 0, errEvent
	}
	var n float64
	if json.Unmarshal(raw, &n) != nil || n < 0 || math.IsInf(n, 0) || math.IsNaN(n) {
		return 0, errEvent
	}
	return n, nil
}
func timestampValue(o object, key string, nullable bool) error {
	if nullable && bytes.Equal(bytes.TrimSpace(o[key]), []byte("null")) {
		return nil
	}
	v, e := stringValue(o, key, true)
	if e != nil {
		return e
	}
	if _, e = time.Parse(time.RFC3339Nano, v); e != nil {
		return errEvent
	}
	return nil
}
func validID(id string) bool {
	if len(id) == 0 || len(id) > 256 {
		return false
	}
	for _, c := range id {
		if c < 33 || c > 126 {
			return false
		}
	}
	return true
}

func parseEvent(raw []byte, id string) (event, error) {
	result := event{}
	o, e := objectValue(raw)
	if e != nil {
		return result, e
	}
	result.ID, e = stringValue(o, "id", true)
	if e != nil || result.ID != id {
		return result, errEvent
	}
	kind, e := stringValue(o, "type", true)
	if e != nil || kind != "meeting.completed" {
		return result, errEvent
	}
	version, e := stringValue(o, "schema_version", true)
	if e != nil {
		return result, errEvent
	}
	parts := strings.Split(version, ".")
	if len(parts) < 2 || len(parts) > 3 || parts[0] != "1" {
		return result, errEvent
	}
	for _, p := range parts[1:] {
		if p == "" {
			return result, errEvent
		}
		for _, c := range p {
			if c < '0' || c > '9' {
				return result, errEvent
			}
		}
		if _, e = strconv.ParseUint(p, 10, 32); e != nil {
			return result, errEvent
		}
	}
	if e = timestampValue(o, "timestamp", false); e != nil {
		return result, e
	}
	if test, ok := o["test"]; ok {
		if bytes.Equal(bytes.TrimSpace(test), []byte("null")) || json.Unmarshal(test, &result.Test) != nil {
			return result, errEvent
		}
	}
	if _, ok := o["related_event_id"]; ok {
		v, e := stringValue(o, "related_event_id", true)
		if e != nil || !validID(v) {
			return result, errEvent
		}
	}
	d, e := objectValue(o["data"])
	if e != nil {
		return result, e
	}
	scope, e := stringValue(d, "content_scope", true)
	if e != nil || (scope != "transcript" && scope != "summary" && scope != "transcript_and_summary") {
		return result, errEvent
	}
	m, e := objectValue(d["meeting"])
	if e != nil {
		return result, e
	}
	if _, e = stringValue(m, "id", true); e != nil {
		return result, e
	}
	if _, e = stringValue(m, "title", false); e != nil {
		return result, e
	}
	if e = timestampValue(m, "started_at", true); e != nil {
		return result, e
	}
	if e = timestampValue(m, "ended_at", true); e != nil {
		return result, e
	}
	if _, e = numberValue(m, "duration_seconds", true); e != nil {
		return result, e
	}
	langs, e := arrayValue(m, "languages")
	if e != nil {
		return result, e
	}
	for _, v := range langs {
		var s string
		if bytes.Equal(bytes.TrimSpace(v), []byte("null")) || json.Unmarshal(v, &s) != nil {
			return result, errEvent
		}
	}
	speakers, e := arrayValue(d, "speakers")
	if e != nil {
		return result, e
	}
	for _, v := range speakers {
		speaker, e := objectValue(v)
		if e != nil {
			return result, e
		}
		for _, k := range []string{"id", "label"} {
			if _, e = stringValue(speaker, k, true); e != nil {
				return result, e
			}
		}
		if e = nullableString(speaker, "name"); e != nil {
			return result, e
		}
	}
	source, e := objectValue(d["source"])
	if e != nil {
		return result, e
	}
	for _, k := range []string{"app", "app_version", "platform"} {
		if _, e = stringValue(source, k, true); e != nil {
			return result, e
		}
	}
	hasTranscript := scope == "transcript" || scope == "transcript_and_summary"
	hasSummary := scope == "summary" || scope == "transcript_and_summary"
	if hasTranscript {
		t, e := objectValue(d["transcript"])
		if e != nil {
			return result, e
		}
		if _, e = stringValue(t, "text", false); e != nil {
			return result, e
		}
		turns, e := arrayValue(t, "turns")
		if e != nil {
			return result, e
		}
		for _, v := range turns {
			turn, e := objectValue(v)
			if e != nil {
				return result, e
			}
			if e = nullableString(turn, "speaker_id"); e != nil {
				return result, e
			}
			start, e := numberValue(turn, "start_seconds", false)
			if e != nil {
				return result, e
			}
			end, e := numberValue(turn, "end_seconds", false)
			if e != nil || end < start {
				return result, errEvent
			}
			if _, e = stringValue(turn, "text", false); e != nil {
				return result, e
			}
		}
	} else if _, ok := d["transcript"]; ok {
		return result, errEvent
	}
	if hasSummary {
		s, e := objectValue(d["summary"])
		if e != nil {
			return result, e
		}
		if _, e = stringValue(s, "text", false); e != nil {
			return result, e
		}
		items, e := arrayValue(d, "action_items")
		if e != nil {
			return result, e
		}
		for _, v := range items {
			item, e := objectValue(v)
			if e != nil {
				return result, e
			}
			if _, e = stringValue(item, "text", false); e != nil {
				return result, e
			}
			raw, ok := item["completed"]
			var completed bool
			if !ok || bytes.Equal(bytes.TrimSpace(raw), []byte("null")) || json.Unmarshal(raw, &completed) != nil {
				return result, errEvent
			}
		}
	} else {
		for _, k := range []string{"summary", "action_items"} {
			if _, ok := d[k]; ok {
				return result, errEvent
			}
		}
	}
	return result, nil
}
