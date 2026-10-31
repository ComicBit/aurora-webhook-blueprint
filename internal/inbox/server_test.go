package inbox

import (
	"context"
	"net"
	"net/http"
	"testing"
	"time"
)

func TestServerSignedDeliveryAndGracefulStop(t *testing.T) {
	store := testStore(t)
	listener, e := net.Listen("tcp", "127.0.0.1:0")
	if e != nil {
		t.Fatal(e)
	}
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	done := make(chan error, 1)
	go func() { done <- serveListener(ctx, listener, store, Secrets{Key: testKey}) }()
	transport := &http.Transport{}
	defer transport.CloseIdleConnections()
	client := &http.Client{Transport: transport, Timeout: 5 * time.Second}
	request := signedRequest("evt_server", synthetic("evt_server", true), time.Now())
	request.URL.Host = listener.Addr().String()
	request.RequestURI = ""
	response, e := client.Do(request)
	if e != nil {
		t.Fatal(e)
	}
	response.Body.Close()
	if response.StatusCode != http.StatusNoContent {
		t.Fatalf("HTTP response: %d", response.StatusCode)
	}
	cancel()
	select {
	case e := <-done:
		if e != nil {
			t.Fatal(e)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("server did not shut down")
	}
	records, e := store.Status()
	if e != nil || len(records) != 1 || records[0].State != "test" {
		t.Fatalf("durable ACK: %+v %v", records, e)
	}
}
