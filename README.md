To run the proxy finder, download rust, clone this repo and run:

```bash
cargo run -r
```

# How this works:

Every node in the Monero P2P network has a self-assigned 64-bit identifier called a peer_id. This peer_id is 
randomly generated at startup and stays static until the node is shutdown[^1].

The peer_id is shared in handshake (request and response) and ping (just response) messages. When a node does a handshake, 
the peer_id it returns, with high probability, should not match with any other peers on the network. For proxy
nodes this is the same, the peer_id they return will be unique, and it will also be constant across connection attempts.

However, when a ping is sent to a proxy node, they return a peer_id that does not match the one they sent during the 
handshake. This peer_id is going to be called the inner peer_id compared to the outer one they give during a handshake. 
There are two classes of proxy nodes that have been detected:

- Class A: these nodes have inner peer_ids that directly match another real node's peer_id. These nodes are not a part 
  of the big subnets and are the single IP addresses.

- Class B: these nodes have inner peer_ids that are often shared among many other class B proxy nodes but are not shared 
  by a reachable real node. These nodes are the nodes in the big subnets.

Class A proxy nodes are very clearly proxying their requests to other public nodes not under their own control. We know
these nodes are not using the default Monero node as their peer_id changes, we also know that the chance of two nodes 
sharing peer_ids is incredibly small. The only conclusion is that these nodes must be proxying requests to other public
nodes but intercepting the handshake and handling that themselves.

Class B proxy nodes are almost certainly proxying requests to unreachable nodes under their own control. Although we 
cannot connect to the real nodes the requests actually go to, we still know these nodes are not running the default 
Monero node. We also know that there are significant overlaps in the inner peer_ids of these nodes showing they are connected. 
Another thing shared during a handshake response message is a list of peers that the node knows about, to propagate 
addresses around the network, class B proxy nodes only share the addresses of other class B proxy nodes.

We can assume that these proxy nodes are trying to overcome Dandelion++ and spy on transactions due to the lack of other
reasons for an entity to run proxy nodes.

---

[^1]: In privacy network zones (Tor, i2p) every peer sets the same peer_id of 1.


