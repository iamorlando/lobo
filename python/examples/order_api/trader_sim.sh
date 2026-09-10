#!/usr/bin/env bash
# Run several copies against the same server to create competing orders.
server=${1:-http://127.0.0.1:8000}

while true; do
    side=buy
    if (( RANDOM % 2 )); then side=sell; fi
    quantity=$((1 + RANDOM % 100))
    price=$((9950 + RANDOM % 101)) # Integer cents: $99.50 to $100.50.

    # Fill what crosses; leave the rest resting in the book.
    curl --silent --show-error --fail-with-body --write-out '\n' \
        "$server/api/books/TRADER/orders" \
        -H 'Content-Type: application/json' \
        -d "{\"op\":\"fill\",\"order\":{\"type\":\"limit\",\"id\":\"$(uuidgen)\",\"side\":\"$side\",\"quantity\":$quantity,\"price\":$price}}"
    sleep 0.1
done
