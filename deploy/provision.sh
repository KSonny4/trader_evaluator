#!/usr/bin/env bash
set -euo pipefail

# Provision an EC2 instance in eu-west-2 (London) for wallet evaluator.
# Prerequisites: aws cli configured, ssh key pair created.

REGION="eu-west-2"
INSTANCE_TYPE="t3.micro"
KEY_NAME="${KEY_NAME:-trading-bot}"
SECURITY_GROUP_NAME="evaluator-sg"
AMI_ID=""  # Will be resolved below

echo "=== Evaluator EC2 Provisioner ==="
echo "Region: $REGION"
echo "Instance: $INSTANCE_TYPE"
echo "Key: $KEY_NAME"
echo ""

# 1. Get latest Ubuntu 22.04 AMI
echo "Resolving latest Ubuntu 22.04 AMI..."
AMI_ID=$(aws ec2 describe-images \
    --region "$REGION" \
    --owners 099720109477 \
    --filters "Name=name,Values=ubuntu/images/hvm-ssd/ubuntu-jammy-22.04-amd64-server-*" \
              "Name=state,Values=available" \
    --query 'Images | sort_by(@, &CreationDate) | [-1].ImageId' \
    --output text)
echo "AMI: $AMI_ID"

# 2. Create key pair if it doesn't exist
if ! aws ec2 describe-key-pairs --region "$REGION" --key-names "$KEY_NAME" &>/dev/null; then
    echo "Creating SSH key pair '$KEY_NAME'..."
    aws ec2 create-key-pair \
        --region "$REGION" \
        --key-name "$KEY_NAME" \
        --query 'KeyMaterial' \
        --output text > "${KEY_NAME}.pem"
    chmod 400 "${KEY_NAME}.pem"
    echo "Key saved to ${KEY_NAME}.pem"
else
    echo "Key pair '$KEY_NAME' already exists"
fi

# 3. Create security group if it doesn't exist
SG_ID=$(aws ec2 describe-security-groups \
    --region "$REGION" \
    --group-names "$SECURITY_GROUP_NAME" \
    --query 'SecurityGroups[0].GroupId' \
    --output text 2>/dev/null || echo "NONE")

if [ "$SG_ID" = "NONE" ] || [ "$SG_ID" = "None" ]; then
    echo "Creating security group '$SECURITY_GROUP_NAME'..."
    SG_ID=$(aws ec2 create-security-group \
        --region "$REGION" \
        --group-name "$SECURITY_GROUP_NAME" \
        --description "Evaluator - SSH + Prometheus + Web" \
        --query 'GroupId' \
        --output text)

    # SSH access (restrict to your IP in production)
    aws ec2 authorize-security-group-ingress \
        --region "$REGION" \
        --group-id "$SG_ID" \
        --protocol tcp --port 22 --cidr 0.0.0.0/0

    # Prometheus metrics (port 9094 for evaluator)
    aws ec2 authorize-security-group-ingress \
        --region "$REGION" \
        --group-id "$SG_ID" \
        --protocol tcp --port 9094 --cidr 0.0.0.0/0

    # Web interface (port 3000)
    aws ec2 authorize-security-group-ingress \
        --region "$REGION" \
        --group-id "$SG_ID" \
        --protocol tcp --port 3000 --cidr 0.0.0.0/0

    echo "Security group: $SG_ID"
else
    echo "Security group exists: $SG_ID"
fi

# 4. Launch instance
echo ""
echo "Launching EC2 instance..."
INSTANCE_ID=$(aws ec2 run-instances \
    --region "$REGION" \
    --image-id "$AMI_ID" \
    --instance-type "$INSTANCE_TYPE" \
    --key-name "$KEY_NAME" \
    --security-group-ids "$SG_ID" \
    --block-device-mappings '[{"DeviceName":"/dev/sda1","Ebs":{"VolumeSize":10,"VolumeType":"gp3"}}]' \
    --tag-specifications "ResourceType=instance,Tags=[{Key=Name,Value=evaluator}]" \
    --query 'Instances[0].InstanceId' \
    --output text)

echo "Instance: $INSTANCE_ID"
echo "Waiting for instance to be running..."

aws ec2 wait instance-running --region "$REGION" --instance-ids "$INSTANCE_ID"

# 5. Get public IP
PUBLIC_IP=$(aws ec2 describe-instances \
    --region "$REGION" \
    --instance-ids "$INSTANCE_ID" \
    --query 'Reservations[0].Instances[0].PublicIpAddress' \
    --output text)

echo ""
echo "=== Instance Ready ==="
echo "IP:       $PUBLIC_IP"
echo "SSH:      ssh -i ${KEY_NAME}.pem ubuntu@${PUBLIC_IP}"
echo "Next:     ./deploy/setup-server.sh ${PUBLIC_IP} && ./deploy/deploy.sh ${PUBLIC_IP}"
echo ""

# Save connection info
cat > deploy/.server-info <<EOF
SERVER_IP=$PUBLIC_IP
KEY_FILE=${KEY_NAME}.pem
SSH_USER=ubuntu
EOF

echo "Server info saved to deploy/.server-info"
